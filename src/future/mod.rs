pub use self::future::{
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
