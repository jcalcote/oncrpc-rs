use super::*;
use std::{future::Future, sync::Barrier, task::Poll};

fn admission(max_calls: usize, max_bytes: usize) -> Admission {
    Admission::new(AdmissionLimits {
        max_calls,
        max_bytes,
    })
    .unwrap()
}
async fn held<F: Future>(mut future: std::pin::Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "expected capacity wait"
        );
        Poll::Ready(())
    })
    .await;
}

async fn ready<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .expect("admission waiter did not observe its release/close signal")
}

#[test]
fn defaults_and_invalid_limits_are_explicit() {
    assert_eq!(
        AdmissionLimits::default(),
        AdmissionLimits {
            max_calls: 1024,
            max_bytes: 128 * 1024 * 1024
        }
    );
    for limits in [
        AdmissionLimits {
            max_calls: 0,
            max_bytes: 1,
        },
        AdmissionLimits {
            max_calls: 1,
            max_bytes: 0,
        },
        AdmissionLimits {
            max_calls: 0,
            max_bytes: 0,
        },
    ] {
        assert_eq!(
            Admission::new(limits).unwrap_err(),
            AdmissionError::InvalidLimits
        );
    }
}

#[test]
fn call_and_byte_limits_are_checked_independently_at_exact_boundaries() {
    let calls = admission(2, 100);
    let one = calls.try_acquire(1).unwrap();
    let two = calls.try_acquire(1).unwrap();
    assert_eq!(
        calls.try_acquire(0).unwrap_err(),
        AdmissionError::AtCapacity
    );
    assert_eq!(calls.snapshot().calls, 2);
    assert_eq!(calls.snapshot().bytes, 2);
    drop(one);
    drop(two);

    let bytes = admission(100, 7);
    let one = bytes.try_acquire(3).unwrap();
    let two = bytes.try_acquire(4).unwrap();
    assert_eq!(
        bytes.try_acquire(1).unwrap_err(),
        AdmissionError::AtCapacity
    );
    assert_eq!(
        bytes.snapshot(),
        AdmissionSnapshot {
            limits: AdmissionLimits {
                max_calls: 100,
                max_bytes: 7
            },
            calls: 2,
            bytes: 7
        }
    );
    drop(one);
    let replacement = bytes.try_acquire(3).unwrap();
    assert_eq!(bytes.snapshot().bytes, 7);
    drop(two);
    assert_eq!(bytes.snapshot().calls, 1);
    assert_eq!(bytes.snapshot().bytes, 3);
    drop(replacement);
    assert_eq!(bytes.snapshot().calls, 0);
    assert_eq!(bytes.snapshot().bytes, 0);
}

#[test]
fn oversized_requests_remain_permanent_even_when_currently_full() {
    let admission = admission(1, 7);
    let permit = admission.try_acquire(7).unwrap();
    let too_large = AdmissionError::TooLarge {
        required: 8,
        limit: 7,
    };
    assert_eq!(admission.try_acquire(8).unwrap_err(), too_large);
    assert_eq!(
        admission.try_acquire(1).unwrap_err(),
        AdmissionError::AtCapacity
    );
    drop(permit);
    assert_eq!(admission.try_acquire(8).unwrap_err(), too_large);
    assert!(AdmissionError::AtCapacity.is_retryable());
    for error in [
        too_large,
        AdmissionError::Closed,
        AdmissionError::InvalidLimits,
        AdmissionError::InvalidRequest(RuntimeError::ConnectionClosed),
    ] {
        assert!(!error.is_retryable());
    }
}

#[test]
fn zero_byte_calls_still_consume_slots_and_release_exactly_once() {
    let admission = admission(1, 1);
    let permit = admission.try_acquire(0).unwrap();
    assert_eq!(admission.snapshot().calls, 1);
    assert_eq!(admission.snapshot().bytes, 0);
    assert_eq!(
        admission.try_acquire(0).unwrap_err(),
        AdmissionError::AtCapacity
    );
    drop(permit);
    assert_eq!(admission.snapshot().calls, 0);
    assert_eq!(admission.snapshot().bytes, 0);
    drop(admission.try_acquire(1).unwrap());
    assert_eq!(admission.snapshot().calls, 0);
}

#[test]
fn compatibility_limits_and_checked_arithmetic_do_not_wrap() {
    let admission = Admission::unlimited();
    assert_eq!(
        admission.snapshot().limits,
        AdmissionLimits {
            max_calls: usize::MAX,
            max_bytes: usize::MAX
        }
    );
    let permit = admission.try_acquire(usize::MAX).unwrap();
    assert_eq!(
        admission.try_acquire(1).unwrap_err(),
        AdmissionError::AtCapacity
    );
    let zero = admission.try_acquire(0).unwrap();
    assert_eq!(admission.snapshot().bytes, usize::MAX);
    drop(permit);
    drop(zero);
    assert_eq!(admission.snapshot().bytes, 0);
    let full_calls = State {
        limits: admission.snapshot().limits,
        calls: usize::MAX,
        bytes: 0,
        closed: false,
    };
    assert_eq!(full_calls.check(0), Err(AdmissionError::AtCapacity));
}

#[test]
fn concurrent_admission_cannot_oversubscribe_either_counter() {
    let admission = admission(5, 15);
    let start = Arc::new(Barrier::new(17));
    let mut workers = Vec::new();
    for _ in 0..16 {
        let start = start.clone();
        let admission = admission.clone();
        workers.push(std::thread::spawn(move || {
            start.wait();
            // Returning the permit keeps it owned by the join result until all
            // competing attempts have finished, independent of thread timing.
            admission.try_acquire(3)
        }));
    }
    start.wait();
    let permits = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(permits.iter().filter(|result| result.is_ok()).count(), 5);
    assert!(
        permits
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| *error == AdmissionError::AtCapacity)
    );
    assert_eq!(admission.snapshot().calls, 5);
    assert_eq!(admission.snapshot().bytes, 15);
    drop(permits);
    assert_eq!(admission.snapshot().calls, 0);
    assert_eq!(admission.snapshot().bytes, 0);
}

#[tokio::test(start_paused = true)]
async fn dropping_a_permit_wakes_a_registered_wait_without_reserving_capacity() {
    let admission = admission(1, 8);
    let permit = admission.try_acquire(8).unwrap();
    let mut wait = Box::pin(admission.wait_for_capacity(8));
    held(wait.as_mut()).await;
    assert_eq!(admission.snapshot().calls, 1);
    assert_eq!(admission.snapshot().bytes, 8);
    // Move the permit across an OS thread: releasing it must wake an async waiter.
    std::thread::spawn(move || drop(permit)).join().unwrap();
    ready(wait).await.unwrap();
    assert_eq!(admission.snapshot().calls, 0);
    assert_eq!(admission.snapshot().bytes, 0);
    let competitor = admission.try_acquire(8).unwrap();
    assert_eq!(
        admission.try_acquire(8).unwrap_err(),
        AdmissionError::AtCapacity
    );
    drop(competitor);
}

#[tokio::test(start_paused = true)]
async fn readiness_may_be_observed_by_multiple_waiters_without_a_hidden_reservation() {
    let admission = admission(1, 8);
    let permit = admission.try_acquire(8).unwrap();
    let mut waits = (0..3)
        .map(|_| Box::pin(admission.wait_for_capacity(8)))
        .collect::<Vec<_>>();
    for wait in &mut waits {
        held(wait.as_mut()).await;
    }
    drop(permit);
    for wait in waits {
        ready(wait).await.unwrap();
    }
    assert_eq!(admission.snapshot().calls, 0);
    let permit = admission.try_acquire(8).unwrap();
    assert_eq!(
        admission.try_acquire(0).unwrap_err(),
        AdmissionError::AtCapacity
    );
    drop(permit);
}

#[tokio::test(start_paused = true)]
async fn close_wakes_every_waiter_but_preserves_busy_accounting_until_drop() {
    let admission = admission(1, 8);
    let permit = admission.try_acquire(7).unwrap();
    let mut waits = (0..3)
        .map(|_| Box::pin(admission.wait_for_capacity(1)))
        .collect::<Vec<_>>();
    for wait in &mut waits {
        held(wait.as_mut()).await;
    }
    admission.close();
    admission.close();
    for wait in waits {
        assert_eq!(ready(wait).await.unwrap_err(), AdmissionError::Closed);
    }
    assert_eq!(
        admission.try_acquire(0).unwrap_err(),
        AdmissionError::Closed
    );
    assert_eq!(
        ready(admission.wait_for_capacity(8)).await.unwrap_err(),
        AdmissionError::Closed
    );
    assert_eq!(admission.snapshot().calls, 1);
    assert_eq!(admission.snapshot().bytes, 7);
    drop(permit);
    assert_eq!(admission.snapshot().calls, 0);
    assert_eq!(admission.snapshot().bytes, 0);
    assert_eq!(
        admission.try_acquire(1).unwrap_err(),
        AdmissionError::Closed
    );
}

#[tokio::test(start_paused = true)]
async fn waiting_rejects_impossible_requests_and_canceling_a_wait_owns_nothing() {
    let admission = admission(1, 8);
    assert_eq!(
        ready(admission.wait_for_capacity(9)).await.unwrap_err(),
        AdmissionError::TooLarge {
            required: 9,
            limit: 8
        }
    );
    ready(admission.wait_for_capacity(8)).await.unwrap();
    let permit = admission.try_acquire(8).unwrap();
    let mut wait = Box::pin(admission.wait_for_capacity(1));
    held(wait.as_mut()).await;
    drop(wait);
    assert_eq!(admission.snapshot().calls, 1);
    drop(permit);
    assert_eq!(admission.snapshot().calls, 0);
    assert_eq!(admission.snapshot().bytes, 0);
}
