use std::time::Duration;

pub trait Cancel {
    /// If not already completed, signals that the consumer is no longer
    /// interested in the result of the future.
    fn cancel(self);
}

// TODO: Switch C to an associated type. Gated on:
// - https://github.com/rust-lang/rust/issues/18178
// - https://github.com/rust-lang/rust/issues/17956
pub trait Future<T, C: Cancel> {
    /// When the future is complete, call the supplied function with the
    /// value.
    fn receive<F: FnOnce(FutureResult<T>) + Send>(self, cb: F) -> C;
}

pub trait SyncFuture<T> {
    /// Get the value from the future, blocking if necessary.
    fn take(self) -> FutureResult<T>;

    fn take_timed(self, timeout: Duration) -> FutureResult<T>;
}

pub type FutureResult<T> = Result<T, FutureError>;

#[deriving(Show)]
pub struct FutureError {
    pub kind: FutureErrorKind,
    pub desc: &'static str,
}

impl FutureError {
    pub fn is_cancelation_error(&self) -> bool {
        match self.kind {
            CancelationError => true,
            _ => false,
        }
    }

    pub fn is_execution_error(&self) -> bool {
        match self.kind {
            ExecutionError => true,
            _ => false,
        }
    }
}

#[deriving(Show)]
pub enum FutureErrorKind {
    ExecutionError,
    CancelationError,
}
