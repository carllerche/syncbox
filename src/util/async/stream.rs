use util::async::{
    self,
    receipt,
    Async,
    Pair,
    Future,
    Complete,
    Cancel,
    Receipt,
    AsyncResult,
    AsyncError
};
use super::core::{self, Core};
use std::fmt;

/*
 *
 * ===== Stream =====
 *
 */

#[unsafe_no_drop_flag]
#[must_use = "streams are lazy and do nothing unless consumed"]
pub struct Stream<T: Send, E: Send> {
    core: Option<Core<Head<T, E>, E>>,
}

/// Convenience type alias for the realized head of a stream
pub type Head<T, E> = Option<(T, Stream<T, E>)>;

// Shorthand for the core type for Streams
type StreamCore<T, E> = Core<Head<T, E>, E>;

impl<T: Send, E: Send> Stream<T, E> {

    /// Creates a new `Stream`, returning it with the associated `Sender`.
    pub fn pair() -> (Sender<T, E>, Stream<T, E>) {
        let core = Core::new();
        let stream = Stream { core: Some(core.clone()) };

        (Sender { core: Some(core) }, stream)
    }

    /// Asyncronously collects the items from the `Stream`, returning them sorted by order of
    /// arrival.
    pub fn collect(self) -> Future<Vec<T>, E> {
        let buffer = Vec::new();
        self.reduce(buffer, |mut vec, item| { vec.push(item); return vec })
    }

    /// Synchronously iterate over the `Stream`
    pub fn iter(mut self) -> StreamIter<T, E> {
        StreamIter { core: Some(core::take(&mut self.core)) }
    }

    /*
     *
     * ===== Computation Builders =====
     *
     */

    pub fn each<F: Fn(T) + Send>(self, f: F) -> Future<(), E> {
        let (complete, ret) = Future::pair();

        complete.receive(move |res| {
            if let Ok(complete) = res {
                self.do_each(f, complete);
            }
        });

        ret
    }

    // Perform the iteration
    fn do_each<F: Fn(T) + Send>(self, f: F, complete: Complete<(), E>) {
        self.receive(move |head| {
            match head {
                Ok(Some((v, rest))) => {
                    f(v);
                    rest.do_each(f, complete);
                }
                Ok(None) => {
                    complete.complete(());
                }
                Err(AsyncError::ExecutionError(e)) => {
                    complete.fail(e);
                }
                _ => {}
            }
        });
    }

    pub fn filter<F: Fn(&T) -> bool + Send>(self, _f: F) -> Stream<T, E> {
        unimplemented!();
    }

    pub fn map<F: Fn(T) -> U + Send, U: Send>(self, f: F) -> Stream<U, E> {
        self.handle(move |res| {
            // Map the result
            res.map(move |head| {
                // Map the option
                head.map(move |(v, rest)| {
                    (f(v), rest.map(f))
                })
            })
        }).as_stream()
    }

    pub fn reduce<F: Fn(U, T) -> U + Send, U: Send>(self, init: U, f: F) -> Future<U, E> {
        self.handle(move |res| {
            match res {
                Ok(Some((v, rest))) => rest.reduce(f(init, v), f),
                Ok(None) => Future::of(init),
                Err(AsyncError::ExecutionError(e)) => Future::error(e),
                _ => Future::canceled(),
            }
        })
    }

    pub fn take(self, n: u64) -> Stream<T, E> {
        if n == 0 {
            Future::of(None).as_stream()
        } else {
            self.handle(move |res| {
                // Map the result
                res.map(move |head| {
                    // Map the option
                    head.map(move |(v, rest)| {
                        (v, rest.take(n - 1))
                    })
                })
            }).as_stream()
        }
    }

    pub fn take_while<F>(self, _f: F) -> Stream<T, E>
            where F: Fn(&T) -> bool + Send {
        unimplemented!();
    }

    pub fn take_until<A>(self, cond: A) -> Stream<T, E>
            where A: Async<Error=E> {

        async::select((cond, self))
            .and_then(move |(i, (cond, stream))| {
                if i == 0 {
                    Ok(None)
                } else {
                    match stream.expect() {
                        Ok(Some((v, rest))) => {
                            Ok(Some((v, rest.take_until(cond))))
                        }
                        _ => Ok(None),
                    }
                }
            }).as_stream()
    }

    /*
     *
     * ===== Internal Helpers =====
     *
     */

    fn from_core(core: StreamCore<T, E>) -> Stream<T, E> {
        Stream { core: Some(core) }
    }
}

impl<T: Send, E: Send> Async for Stream<T, E> {

    type Value  = Head<T, E>;
    type Error  = E;
    type Cancel = Receipt<Stream<T, E>>;

    fn is_ready(&self) -> bool {
        core::get(&self.core).consumer_is_ready()
    }

    fn is_err(&self) -> bool {
        core::get(&self.core).consumer_is_err()
    }

    fn poll(mut self) -> Result<AsyncResult<Head<T, E>, E>, Stream<T, E>> {
        let core = core::take(&mut self.core);

        match core.consumer_poll() {
            Some(res) => Ok(res),
            None => Err(Stream { core: Some(core) })
        }
    }

    fn ready<F: FnOnce(Stream<T, E>) + Send + 'static>(mut self, f: F) -> Receipt<Stream<T, E>> {
        let core = core::take(&mut self.core);

        match core.consumer_ready(move |core| f(Stream::from_core(core))) {
            Some(count) => receipt::new(core, count),
            None => receipt::none(),
        }
    }

    fn await(mut self) -> AsyncResult<Head<T, E>, E> {
        core::take(&mut self.core).consumer_await()
    }
}

impl<T: Send, E: Send> Pair for Stream<T, E> {
    type Tx = Sender<T, E>;

    fn pair() -> (Sender<T, E>, Stream<T, E>) {
        Stream::pair()
    }
}

impl<T: Send, E: Send> fmt::Debug for Stream<T, E> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Stream<?>")
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for Stream<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            core::take(&mut self.core).cancel();
        }
    }
}

impl<T: Send, E: Send> Cancel<Stream<T, E>> for Receipt<Stream<T, E>> {
    fn cancel(self) -> Option<Stream<T, E>> {
        let (core, count) = receipt::parts(self);

        if !core.is_some() {
            return None;
        }

        if core::get(&core).consumer_ready_cancel(count) {
            return Some(Stream { core: core });
        }

        None
    }
}

/*
 *
 * ===== Sender =====
 *
 */

/// The sending half of `Stream::pair()`. Can only be owned by a single task at
/// a time.
pub struct Sender<T: Send, E: Send> {
    core: Option<StreamCore<T, E>>,
}

impl<T: Send, E: Send> Sender<T, E> {

    // TODO: This fn would be nice to have, but isn't possible with the current
    // implementation of `send()` which requires the value slot of `Stream` to
    // always be available.
    //
    // I initially thought that `try_send` would be possible to implement if
    // the consumer was currently waiting for a value, but this doesn't work in
    // sync mode since the value is temporarily stored in the stream's value
    // slot and there would be a potential race condition.
    //
    // pub fn try_send(&self, val: T) -> Result<(), T>;

    /// Attempts to send a value to its `Stream`. Consumes self and returns a
    /// future representing the operation completing successfully and interest
    /// in the next value being expressed.
    pub fn send(mut self, val: T) -> BusySender<T, E> {
        let core = core::take(&mut self.core);
        let val = Some((val, Stream { core: Some(core.clone()) }));

        // Complete the value
        core.complete(Ok(val), false);
        BusySender { core: Some(core) }
    }

    pub fn fail(mut self, err: E) {
        core::take(&mut self.core).complete(Err(AsyncError::wrap(err)), true);
    }

    /*
     *
     * ===== Internal Helpers =====
     *
     */

    fn from_core(core: StreamCore<T, E>) -> Sender<T, E> {
        Sender { core: Some(core) }
    }
}

impl<T: Send, E: Send> Async for Sender<T, E> {

    type Value  = Sender<T, E>;
    type Error  = ();
    type Cancel = Receipt<Sender<T, E>>;

    fn is_ready(&self) -> bool {
        core::get(&self.core).producer_is_ready()
    }

    fn is_err(&self) -> bool {
        core::get(&self.core).producer_is_err()
    }

    fn poll(mut self) -> Result<AsyncResult<Sender<T, E>, ()>, Sender<T, E>> {
        debug!("Sender::poll; is_ready={}", self.is_ready());

        let core = core::take(&mut self.core);

        match core.producer_poll() {
            Some(res) => Ok(res.map(Sender::from_core)),
            None => Err(Sender { core: Some(core) })
        }
    }

    fn ready<F: FnOnce(Sender<T, E>) + Send + 'static>(mut self, f: F) -> Receipt<Sender<T, E>> {
        core::take(&mut self.core).producer_ready(move |core| {
            f(Sender::from_core(core))
        });

        receipt::none()
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for Sender<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            debug!("Sender::drop(); cancelling future");
            // Get the core
            let core = core::take(&mut self.core);
            core.complete(Ok(None), true);
        }
    }
}

/*
 *
 * ===== BusySender =====
 *
 */

pub struct BusySender<T: Send, E: Send> {
    core: Option<StreamCore<T, E>>,
}

impl<T: Send, E: Send> BusySender<T, E> {
    /*
     *
     * ===== Internal Helpers =====
     *
     */

    fn from_core(core: StreamCore<T, E>) -> BusySender<T, E> {
        BusySender { core: Some(core) }
    }
}

impl<T: Send, E: Send> Async for BusySender<T, E> {
    type Value = Sender<T, E>;
    type Error = ();
    type Cancel = Receipt<BusySender<T, E>>;

    fn is_ready(&self) -> bool {
        core::get(&self.core).consumer_is_ready()
    }

    fn is_err(&self) -> bool {
        core::get(&self.core).consumer_is_err()
    }

    fn poll(mut self) -> Result<AsyncResult<Sender<T, E>, ()>, BusySender<T, E>> {
        debug!("Sender::poll; is_ready={}", self.is_ready());

        let core = core::take(&mut self.core);

        match core.producer_poll() {
            Some(res) => Ok(res.map(Sender::from_core)),
            None => Err(BusySender { core: Some(core) })
        }
    }

    fn ready<F: FnOnce(BusySender<T, E>) + Send + 'static>(mut self, f: F) -> Receipt<BusySender<T, E>> {
        core::take(&mut self.core).producer_ready(move |core| {
            f(BusySender::from_core(core))
        });

        receipt::none()
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for BusySender<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            let core = core::take(&mut self.core);

            core.producer_ready(|core| {
                if core.producer_is_ready() {
                    core.complete(Ok(None), true);
                }
            });
        }
    }
}

impl<T: Send, E: Send> fmt::Debug for Sender<T, E> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Sender<?>")
    }
}

/*
 *
 * ===== Receipt<Sender<T, E>> =====
 *
 */

impl<T: Send, E: Send> Cancel<Sender<T, E>> for Receipt<Sender<T, E>> {
    fn cancel(self) -> Option<Sender<T, E>> {
        None
    }
}

/*
 *
 * ===== Receipt<BusySender<T, E>> =====
 *
 */

impl<T: Send, E: Send> Cancel<BusySender<T, E>> for Receipt<BusySender<T, E>> {
    fn cancel(self) -> Option<BusySender<T, E>> {
        None
    }
}

/*
 *
 * ===== StreamIter =====
 *
 */

#[unsafe_no_drop_flag]
pub struct StreamIter<T: Send, E: Send> {
    core: Option<StreamCore<T, E>>,
}

impl<T: Send, E: Send> Iterator for StreamIter<T, E> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        use std::mem;

        match core::get(&self.core).consumer_await() {
            Ok(Some((h, mut rest))) => {
                mem::replace(&mut self.core, Some(core::take(&mut rest.core)));
                Some(h)
            }
            Ok(None) => {
                let _ = core::take(&mut self.core);
                None
            }
            Err(_) => unimplemented!(),
        }
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for StreamIter<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            core::take(&mut self.core).cancel();
        }
    }
}

pub fn from_core<T: Send, E: Send>(core: StreamCore<T, E>) -> Stream<T, E> {
    Stream { core: Some(core) }
}
