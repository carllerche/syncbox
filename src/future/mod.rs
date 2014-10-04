pub use self::future::{Future, SyncFuture};
pub use self::val::{FutureVal, Completer, future};

mod future;
mod val;
