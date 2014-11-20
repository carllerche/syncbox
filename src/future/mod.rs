pub use self::future::{
    Cancel,
    Future,
    SyncFuture,
    FutureResult,
    FutureError,
    FutureErrorKind,
};
pub use self::val::{
    FutureVal,
    future
};

mod future;
pub mod val;
