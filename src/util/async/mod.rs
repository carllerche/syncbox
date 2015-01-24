//! # Futures & Streams
//!
//! The async module contains utilities for managing asynchronous computations.
//! These utilities are primarily based around `Future` and `Stream` types as
//! well as functions that allow describing computations on these types.
//!
//! ## Future
//!
//! A future represents a value that will be provided sometime in the future.
//! The value may be computed concurrently in another thread or may be provided
//! upon completion of an asynchronous callback. The abstraction allows
//! describing computations to perform on the value once it is realized as well
//! as how to handle errors.
//!
//! One way to think of a Future is as a Result where the value is
//! asynchronously computed.
//!
//! ## Stream
//!
//! A stream is like a feature, except that instead of representing a single
//! value, it represents a sequence of values.
//!
#![experimental]

pub use self::future::{Future, Complete};
pub use self::stream::{Stream, StreamIter, Generate};
pub use self::join::{join, ToJoin};

use util::Run;

use std::fmt;
use self::AsyncError::*;

// ## TODO
//
// * Switch generics to where clauses
//   - rust-lang/rust#20300 (T::Foo resolution)
//
// * Allow Async::or & Async::or_else to change the error type
//
// * Improve performance / reduce allocations

mod core;
mod future;
mod join;
mod stream;

pub trait Async : Send + Sized {
    type Value: Send;
    type Error: Send;

    /// Returns true if `take` will succeed.
    fn is_ready(&self) -> bool;

    /// Get the underlying value if present
    fn poll(self) -> Result<AsyncResult<Self::Value, Self::Error>, Self>;

    /// Invokes the given function when the Async instance is ready to be
    /// consumed.
    fn ready<F>(self, f: F) where F: FnOnce(Self) + Send;

    /// Invoke the callback with the resolved `Async` result.
    fn receive<F>(self, f: F)
            where F: FnOnce(AsyncResult<Self::Value, Self::Error>) + Send {
        self.ready(move |async| {
            match async.poll() {
                Ok(res) => f(res),
                Err(_) => panic!("ready callback invoked but is not actually ready"),
            }
        });
    }

    /// Invoke the callback on the specified `Run` with the resolved `Async`
    /// result.
    fn receive_defer<F, R>(self, f: F, run: R)
            where F: FnOnce(AsyncResult<Self::Value, Self::Error>) + Send,
                  R: Run {
        // TODO: It should be possible for the future itself to behave as a
        // Run task in order to reduce an allocation.
        self.receive(move |res| {
            run.run(move || f(res));
        });
    }

    fn await(self) -> AsyncResult<Self::Value, Self::Error> {
        use std::sync::mpsc::channel;

        let (tx, rx) = channel();

        self.receive(move |res| tx.send(res).ok().expect("receiver thread died"));
        rx.recv().ok().expect("async disappeared without a trace")
    }

    /*
     *
     * ===== Computation Builders =====
     *
     */

    // Is this needed?
    fn handle<F, U: Async>(self, cb: F) -> Future<U::Value, U::Error>
            where F: FnOnce(AsyncResult<Self::Value, Self::Error>) -> U + Send,
                  U::Value: Send, U::Error: Send {
        // TODO: Currently a naive implementation. Improve it by reducing
        // required allocations
        let (ret, complete) = Future::pair();

        complete.receive(move |c| {
            if let Ok(complete) = c {
                self.receive(move |v| {
                    cb(v).receive(move |res| {
                        match res {
                            Ok(u) => complete.complete(u),
                            Err(e) => {
                                if let AsyncError::ExecutionError(e) = e {
                                    complete.fail(e);
                                }
                            }
                        }
                    });
                });
            }
        });

        ret
    }

    /// If the future completes successfully, returns the complection of
    /// `next`.
    fn and<U: Async<Error=Self::Error>>(self, next: U) -> Future<U::Value, Self::Error> {
        self.and_then(move |_| next)
    }

    /// Also handles the Future::map case
    fn and_then<F, U: Async<Error=Self::Error>>(self, f: F) -> Future<U::Value, Self::Error>
            where F: FnOnce(Self::Value) -> U + Send,
                  U::Value: Send {
        let (ret, complete) = Future::pair();

        complete.receive(move |c| {
            if let Ok(complete) = c {
                self.receive(move |res| {
                    match res {
                        Ok(v) => {
                            f(v).receive(move |res| {
                                match res {
                                    Ok(u) => complete.complete(u),
                                    Err(ExecutionError(e)) => complete.fail(e),
                                    _ => {}
                                }
                            });
                        }
                        Err(ExecutionError(e)) => complete.fail(e),
                        _ => {}
                    }
                });
            }
        });

        ret
    }

    fn or<A>(self, alt: A) -> Future<Self::Value, Self::Error>
            where A: Async<Value=Self::Value, Error=Self::Error> {
        self.or_else(move |_| alt)
    }

    fn or_else<F, A>(self, f: F) -> Future<Self::Value, Self::Error>
            where F: FnOnce(AsyncError<Self::Error>) -> A + Send,
                  A: Async<Value=Self::Value, Error=Self::Error> {

        let (ret, complete) = Future::pair();

        complete.receive(move |c| {
            if let Ok(complete) = c {
                self.receive(move |res| {
                    match res {
                        Ok(v) => complete.complete(v),
                        Err(e) => {
                            f(e).receive(move |res| {
                                match res {
                                    Ok(v) => complete.complete(v),
                                    Err(ExecutionError(e)) => complete.fail(e),
                                    _ => {}
                                }
                            });
                        }
                    }
                });
            }
        });

        ret
    }
}

/*
 *
 * ===== Async implementations =====
 *
 */

impl Async for () {
    type Value = ();
    type Error = ();

    fn is_ready(&self) -> bool {
        true
    }

    fn poll(self) -> Result<AsyncResult<(), ()>, ()> {
        Ok(Ok(self))
    }

    fn ready<F: FnOnce(()) + Send>(self, f: F) {
        f(self);
    }

    fn await(self) -> AsyncResult<(), ()> {
        Ok(self)
    }
}

impl<T: Send, E: Send> Async for AsyncResult<T, E> {
    type Value = T;
    type Error = E;

    fn is_ready(&self) -> bool {
        true
    }

    fn poll(self) -> Result<AsyncResult<T, E>, AsyncResult<T, E>> {
        Ok(self)
    }

    fn ready<F: FnOnce(AsyncResult<T, E>) + Send>(self, f: F) {
        f(self);
    }

    fn await(self) -> AsyncResult<T, E> {
        self
    }
}

/*
 *
 * ===== AsyncResult =====
 *
 */

pub type AsyncResult<T, E> = Result<T, AsyncError<E>>;

pub enum AsyncError<E: Send> {
    ExecutionError(E),
    CancellationError,
}

impl<E: Send> AsyncError<E> {
    pub fn wrap(err: E) -> AsyncError<E> {
        AsyncError::ExecutionError(err)
    }

    pub fn canceled() -> AsyncError<E> {
        AsyncError::CancellationError
    }

    pub fn is_cancellation(&self) -> bool {
        match *self {
            AsyncError::CancellationError => true,
            _ => false,
        }
    }

    pub fn is_execution_error(&self) -> bool {
        match *self {
            AsyncError::ExecutionError(..) => true,
            _ => false,
        }
    }

    pub fn unwrap(self) -> E {
        match self {
            AsyncError::ExecutionError(err) => err,
            AsyncError::CancellationError => panic!("unwrapping a cancellation error"),
        }
    }

    pub fn take(self) -> Option<E> {
        match self {
            AsyncError::ExecutionError(err) => Some(err),
            _ => None,
        }
    }
}

impl<E: Send + fmt::Debug> fmt::Display for AsyncError<E> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            AsyncError::ExecutionError(ref e) => write!(fmt, "ExecutionError({:?})", e),
            AsyncError::CancellationError => write!(fmt, "CancellationError"),
        }
    }
}

/*
 *
 * ===== BoxedReceive =====
 *
 */

// Needed to allow virtual dispatch to Receive
trait BoxedReceive<T> : Send {
    fn receive_boxed(self: Box<Self>, val: T);
}

impl<F: FnOnce(T) + Send, T> BoxedReceive<T> for F {
    fn receive_boxed(self: Box<F>, val: T) {
        (*self)(val)
    }
}
