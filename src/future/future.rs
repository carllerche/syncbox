pub trait Future<T> {
    /// When the future is complete, call the supplied function with the
    /// value.
    fn receive<F: FnOnce(T) -> () + Send>(self, cb: F);

    /// If not already completed, signals that the consumer is no longer
    /// interested in the result of the future.
    fn cancel(self);
}

pub trait SyncFuture<T> : Future<T> {
    /// Get the value from the future, blocking if necessary.
    fn take(self) -> T;
}

pub type FutureResult<T> = Result<T, FutureError>;

pub struct FutureError {
    kind: FutureErrorKind,
    desc: &'static str,
}

pub enum FutureErrorKind {
    ExecutionError,
}
