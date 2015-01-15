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
pub use self::stream::{Stream, StreamIter, Produce};
pub use self::join::{join, ToJoin};

use std::fmt;
use self::AsyncError::*;

// ## TODO
//
// * Use associated types. Blocked on:
//   - rust-lang/rust#20543 (nested type constraints)
//   - rust-lang/rust#20540 (associated types & default types)
//
// * Implement Async::cancel()
//
// * All handles go out of scope w/o reading => cancel future
//
// * Improve Async::handle_with (reduce allocations)
//
// * Rename Complete::receive -> ready (same with Produce)
//
// * Consider a default implementation for Async::await()

mod core;
mod future;
mod join;
mod stream;

pub trait Async<T: Send, E: Send> : Send + Sized {
    fn receive<F: FnOnce(AsyncResult<T, E>) + Send>(self, cb: F);

    /*
     *
     * ===== Computation Builders =====
     *
     */

    // Is this needed?
    fn handle<F: FnOnce(AsyncResult<T, E>) -> R + Send, R: Async<U, E2>, U: Send, E2: Send>(self, cb: F) -> Future<U, E2> {
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
    fn and<A: Async<U, E>, U: Send>(self, next: A) -> Future<U, E> {
        self.and_then(move |_| next)
    }

    /// Also handles the Future::map case
    fn and_then<F: FnOnce(T) -> A + Send, A: Async<U, E>, U: Send>(self, f: F) -> Future<U, E> {
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

    fn or<A: Async<T, E>>(self, alt: A) -> Future<T, E> {
        self.or_else(move |_| alt)
    }

    fn or_else<F: FnOnce(AsyncError<E>) -> A + Send, A: Async<T, E>>(self, f: F) -> Future<T, E> {
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

    fn catch<F: FnOnce(AsyncError<E>) -> R + Send, R: Async<T, E2>, E2: Send = AsyncError<()>>(self, cb: F) -> Future<T, E2> {
        let (ret, complete) = Future::pair();

        complete.receive(move |c| {
            if let Ok(complete) = c {
                self.receive(move |res| {
                    match res {
                        Ok(v) => complete.complete(v),
                        Err(err) => {
                            cb(err).receive(move |res| {
                                match res {
                                    Ok(v) => complete.complete(v),
                                    Err(e) => {
                                        if let AsyncError::ExecutionError(e) = e {
                                            complete.fail(e);
                                        }
                                    }
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

impl Async<(), ()> for () {
    fn receive<F: FnOnce(AsyncResult<(), ()>) + Send>(self, f: F) {
        f(Ok(self));
    }
}

impl<T: Send, E: Send> Async<T, E> for AsyncResult<T, E> {
    fn receive<F: FnOnce(AsyncResult<T, E>) + Send>(self, f: F) {
        f(self);
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

impl<E: Send + fmt::Show> fmt::Show for AsyncError<E> {
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
trait BoxedReceive<T: Send, E: Send> : Send {
    fn receive_boxed(self: Box<Self>, val: AsyncResult<T, E>);
}

impl<F: FnOnce(AsyncResult<T, E>) + Send, T: Send, E: Send> BoxedReceive<T, E> for F {
    fn receive_boxed(self: Box<F>, val: AsyncResult<T, E>) {
        (*self)(val)
    }
}
