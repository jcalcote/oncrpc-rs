//! Pre-send admission for a shared RPC connection.
//!
//! Byte weights conservatively account for logical request bytes, descriptors
//! and inline receiver state. They are not a measurement or limit of process RSS.
use crate::RuntimeError;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::Notify;

/// Maximum admitted calls and their combined conservative byte weights.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AdmissionLimits {
    /// Maximum pending calls, including operations retained through cleanup.
    pub max_calls: usize,
    /// Maximum sum of conservative per-call byte weights.
    pub max_bytes: usize,
}
impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            max_calls: 1024,
            max_bytes: 128 * 1024 * 1024,
        }
    }
}

/// Rejection before transferring a request into the connection's pending set.
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum AdmissionError {
    /// Other accepted calls currently occupy the required capacity.
    #[error("RPC admission is currently at capacity")]
    AtCapacity,
    /// This request cannot fit even when the connection is idle.
    #[error("RPC admission requires {required} bytes, exceeding the {limit}-byte limit")]
    TooLarge {
        /// Required logical admission charge.
        required: usize,
        /// Configured byte ceiling.
        limit: usize,
    },
    /// The connection has failed or was explicitly closed.
    #[error("RPC admission is closed")]
    Closed,
    /// A zero call or byte ceiling is not a valid configuration.
    #[error("RPC admission limits must both be greater than zero")]
    InvalidLimits,
    /// The request cannot be represented or violates its wire contract.
    #[error("invalid RPC admission request: {0}")]
    InvalidRequest(#[source] RuntimeError),
}
impl AdmissionError {
    /// Whether this rejection can become eligible when other calls finish.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::AtCapacity)
    }
}

/// Current connection admission, including calls retained through cleanup.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AdmissionSnapshot {
    /// Limits selected at connection construction. Compatibility mode uses `usize::MAX`.
    pub limits: AdmissionLimits,
    /// Accepted operations, including unpolled futures and cleanup owners.
    pub calls: usize,
    /// Sum of the charges belonging to those operations.
    pub bytes: usize,
}

#[derive(Debug)]
struct State {
    limits: AdmissionLimits,
    calls: usize,
    bytes: usize,
    closed: bool,
}
impl State {
    fn check(&self, bytes: usize) -> Result<(), AdmissionError> {
        if self.closed {
            return Err(AdmissionError::Closed);
        }
        if bytes > self.limits.max_bytes {
            return Err(AdmissionError::TooLarge {
                required: bytes,
                limit: self.limits.max_bytes,
            });
        }
        if self
            .calls
            .checked_add(1)
            .is_none_or(|calls| calls > self.limits.max_calls)
            || self
                .bytes
                .checked_add(bytes)
                .is_none_or(|bytes| bytes > self.limits.max_bytes)
        {
            return Err(AdmissionError::AtCapacity);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Inner {
    state: Mutex<State>,
    changed: Notify,
}
impl Inner {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Admission(Arc<Inner>);
impl Admission {
    pub(crate) fn new(limits: AdmissionLimits) -> Result<Self, AdmissionError> {
        if limits.max_calls == 0 || limits.max_bytes == 0 {
            return Err(AdmissionError::InvalidLimits);
        }
        Ok(Self(Arc::new(Inner {
            state: Mutex::new(State {
                limits,
                calls: 0,
                bytes: 0,
                closed: false,
            }),
            changed: Notify::new(),
        })))
    }

    /// Compatibility mode for constructors without explicit admission limits.
    pub(crate) fn unlimited() -> Self {
        Self::new(AdmissionLimits {
            max_calls: usize::MAX,
            max_bytes: usize::MAX,
        })
        .expect("nonzero compatibility limits")
    }

    /// Atomically reserve a call slot and byte weight before ownership transfer.
    pub(crate) fn try_acquire(&self, bytes: usize) -> Result<AdmissionPermit, AdmissionError> {
        let mut state = self.0.state();
        state.check(bytes)?;
        state.calls += 1;
        state.bytes += bytes;
        Ok(AdmissionPermit {
            admission: self.clone(),
            bytes,
        })
    }

    /// Await readiness only. Another caller can consume capacity before a
    /// subsequent try_acquire, so callers must recheck admission themselves.
    /// This future stores no request payload and reserves no slot or bytes.
    pub(crate) async fn wait_for_capacity(&self, bytes: usize) -> Result<(), AdmissionError> {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            // Register before checking: release/close cannot fall into a gap
            // between observing full state and starting to wait.
            changed.as_mut().enable();
            let readiness = { self.0.state().check(bytes) };
            match readiness {
                Ok(()) => return Ok(()),
                Err(AdmissionError::AtCapacity) => changed.await,
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) fn snapshot(&self) -> AdmissionSnapshot {
        let state = self.0.state();
        AdmissionSnapshot {
            limits: state.limits,
            calls: state.calls,
            bytes: state.bytes,
        }
    }

    /// Stop future admission and wake all waiters. Existing owners stay charged
    /// until their permits are dropped at actual completion or quiescence.
    pub(crate) fn close(&self) {
        self.0.state().closed = true;
        self.0.changed.notify_waiters();
    }
}

/// A non-clonable owner of exactly one admitted call and its byte weight.
#[derive(Debug)]
pub(crate) struct AdmissionPermit {
    admission: Admission,
    bytes: usize,
}
impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        {
            let mut state = self.admission.0.state();
            state.calls -= 1;
            state.bytes -= self.bytes;
        }
        self.admission.0.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests;
