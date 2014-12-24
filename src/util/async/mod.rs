pub use self::future::{Future, Producer};

mod future;
mod stream;

// TODO: Switch C to an associated type. Gated on:
// - https://github.com/rust-lang/rust/issues/18178
// - https://github.com/rust-lang/rust/issues/17956
pub trait Async<C: Cancel> : Send {
    // Invoke the callback with the future on completion
    fn ready<F: Send + FnOnce(Self)>(self, cb: F) -> C;
}

pub trait Cancel {
    /// If not already completed, signals that the consumer is no longer
    /// interested in the result of the future.
    fn cancel(self);
}

pub type AsyncResult<T> = Result<T, AsyncError>;

#[deriving(Copy, Show)]
pub struct AsyncError {
    pub kind: AsyncErrorKind,
    pub desc: &'static str,
}

impl AsyncError {
    pub fn is_cancelation(&self) -> bool {
        match self.kind {
            AsyncErrorKind::CancelationError => true,
            _ => false,
        }
    }

    pub fn is_execution_error(&self) -> bool {
        match self.kind {
            AsyncErrorKind::ExecutionError => true,
            _ => false,
        }
    }
}

#[deriving(Copy, Show)]
pub enum AsyncErrorKind {
    ExecutionError,
    CancelationError,
}
