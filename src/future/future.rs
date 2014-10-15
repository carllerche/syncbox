
pub trait Future<T> {
    /// When the future is complete, call the supplied function with the
    /// value.
    fn receive<F: FnOnce(FutureResult<T>) + Send>(self, cb: F);

    /// If not already completed, signals that the consumer is no longer
    /// interested in the result of the future.
    fn cancel(self);
}

pub trait SyncFuture<T> : Future<T> {
    /// Get the value from the future, blocking if necessary.
    fn take(self) -> FutureResult<T>;
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
