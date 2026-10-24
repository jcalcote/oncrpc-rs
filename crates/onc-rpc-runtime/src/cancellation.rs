use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

/// A clonable, one-way request to cancel local RPC processing.
///
/// Cancellation cannot undo remote execution. Use one token per call, or share
/// it deliberately to cancel a group. To recover caller-owned buffers, cancel
/// the token and keep awaiting the call's outcome.
#[derive(Clone, Default)]
pub struct CallCancellation(Arc<Inner>);

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    changed: Notify,
}

impl CallCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request local cancellation. Repeated requests have no further effect.
    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::AcqRel) {
            self.0.changed.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    /// Wait without losing a cancellation racing with waiter registration.
    pub async fn cancelled(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            changed.await;
        }
    }
}

impl PartialEq for CallCancellation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for CallCancellation {}
impl std::fmt::Debug for CallCancellation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}
