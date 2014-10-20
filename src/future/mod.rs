pub use self::future::{
    Cancel,
    Future,
    SyncFuture,
    FutureResult,
    FutureError,
    FutureErrorKind,
    ExecutionError,
    CancelationError,
};
pub use self::val::{
    FutureVal,
    future
};

mod future;
pub mod val;
