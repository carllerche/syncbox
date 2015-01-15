use super::{Async, Stream, AsyncResult, AsyncError};
use super::stream;
use super::core::{Core, OptionCore, FromCore};
use std::fmt;

/* TODO:
 * - Add AsyncVal trait that impls all the various monadic fns
 */

#[unsafe_no_drop_flag]
pub struct Future<T: Send, E: Send> {
    core: OptionCore<T, E, Complete<T, E>>,
}

impl<T: Send, E: Send> Future<T, E> {
    pub fn pair() -> (Future<T, E>, Complete<T, E>) {
        let core = Core::new();
        let future = Future { core: OptionCore::new(core.clone()) };

        (future, Complete { core: OptionCore::new(core) })
    }

    /// Returns an immediately ready future containing the supplied value.
    pub fn of(val: T) -> Future<T, E> {
        Future { core: OptionCore::new(Core::with_value(Ok(val))) }
    }

    /// Returns a future that will immediately fail with the supplied error.
    pub fn error(err: E) -> Future<T, E> {
        let core = Core::with_value(Err(AsyncError::wrap(err)));
        Future { core: OptionCore::new(core) }
    }

    pub fn canceled() -> Future<T, E> {
        let core = Core::with_value(Err(AsyncError::canceled()));
        Future { core: OptionCore::new(core) }
    }

    pub fn lazy<F: FnOnce() -> R + Send, R: Async<T, E>>(f: F) -> Future<T, E> {
        let (future, complete) = Future::pair();

        // Complete the future with the provided function once consumer
        // interest has been registered.
        complete.receive(move |:c: AsyncResult<Complete<T, E>, ()>| {
            if let Ok(c) = c {
                f().receive(move |res| {
                    match res {
                        Ok(v) => c.complete(v),
                        Err(_) => unimplemented!(),
                    }
                });
            }
        });

        future
    }

    pub fn receive<F: FnOnce(AsyncResult<T, E>) + Send>(mut self, f: F) {
        self.core.take().consumer_receive(f);
    }

    pub fn await(mut self) -> AsyncResult<T, E> {
        self.core.take().consumer_await()
    }

    /*
     *
     * ===== Computation Builders =====
     *
     */

    pub fn map<F: FnOnce(T) -> U + Send, U: Send>(self, _f: F) -> Future<U, E> {
        unimplemented!();
    }
}

impl<T: Send, E: Send> Future<Option<(T, Stream<T, E>)>, E> {
    pub fn as_stream(mut self) -> Stream<T, E> {
        stream::from_core(self.core.take())
    }
}

impl<T: Send, E: Send> Async<T, E> for Future<T, E> {
    fn receive<F: FnOnce(AsyncResult<T, E>) + Send>(self, f: F) {
        Future::receive(self, f);
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for Future<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            self.core.take().cancel();
        }
    }
}

#[unsafe_no_drop_flag]
pub struct Complete<T: Send, E: Send> {
    core: OptionCore<T, E, Complete<T, E>>,
}

impl<T: Send, E: Send> Complete<T, E> {
    pub fn complete(mut self, val: T) {
        self.core.take().complete(Ok(val), true);
    }

    pub fn fail(mut self, err: E) {
        self.core.take().complete(Err(AsyncError::wrap(err)), true);
    }

    pub fn receive<F: FnOnce(AsyncResult<Complete<T, E>, ()>) + Send>(mut self, f: F) {
        self.core.take().producer_receive(f);
    }

    pub fn await(mut self) -> AsyncResult<Complete<T, E>, ()> {
        self.core.take().producer_await()
    }
}

impl<T: Send, E: Send> FromCore<T, E> for Complete<T, E> {
    fn from_core(core: Core<T, E, Complete<T, E>>) -> Complete<T, E> {
        Complete { core: OptionCore::new(core) }
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for Complete<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            self.core.take().complete(Err(AsyncError::canceled()), true);
        }
    }
}

impl<T: Send, E: Send> fmt::Show for Complete<T, E> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Complete {{ ... }}")
    }
}
