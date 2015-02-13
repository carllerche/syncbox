use util::async::{self, Async, Future, Cancel, AsyncResult, AsyncError};
use super::core::{self, Core, FromCore};
use std::fmt;

pub type Head<T, E> = Option<(T, Stream<T, E>)>;

#[unsafe_no_drop_flag]
pub struct Stream<T: Send, E: Send> {
    core: Option<Core<Stream<T, E>>>,
}

impl<T: Send, E: Send> Stream<T, E> {
    pub fn pair() -> (Sender<T, E>, Stream<T, E>) {
        let core = Core::new();
        let stream = Stream { core: Some(core.clone()) };

        (Sender { core: Some(core) }, stream)
    }

    pub fn is_ready(&self) -> bool {
        core::get(&self.core).consumer_is_ready()
    }

    pub fn is_err(&self) -> bool {
        core::get(&self.core).consumer_is_err()
    }

    pub fn poll(mut self) -> Result<AsyncResult<Head<T, E>, E>, Stream<T, E>> {
        let core = core::take(&mut self.core);

        match core.consumer_poll() {
            Some(res) => Ok(res),
            None => Err(Stream { core: Some(core) })
        }
    }

    pub fn ready<F: FnOnce(Stream<T, E>) + Send>(mut self, f: F) -> CancelStream<T, E> {
        let core = core::take(&mut self.core);

        match core.consumer_ready(f) {
            Some(count) => CancelStream::new(core, count),
            None => CancelStream::none(),
        }
    }

    pub fn await(mut self) -> AsyncResult<Head<T, E>, E> {
        core::take(&mut self.core).consumer_await()
    }

    pub fn iter(mut self) -> StreamIter<T, E> {
        StreamIter { core: Some(core::take(&mut self.core)) }
    }

    /*
     *
     * ===== Computation Builders =====
     *
     */

    pub fn each<F: Fn(T) + Send>(self, _f: F) -> Future<(), E> {
        unimplemented!();
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
}

impl<T: Send, E: Send> Async for Stream<T, E> {
    type Value = Head<T, E>;
    type Error = E;
    type Cancel = CancelStream<T, E>;

    fn is_ready(&self) -> bool {
        Stream::is_ready(self)
    }

    fn is_err(&self) -> bool {
        Stream::is_err(self)
    }

    fn poll(self) -> Result<AsyncResult<Head<T, E>, E>, Stream<T, E>> {
        Stream::poll(self)
    }

    fn ready<F: FnOnce(Stream<T, E>) + Send>(self, f: F) -> CancelStream<T, E> {
        Stream::ready(self, f)
    }

    fn await(self) -> AsyncResult<Head<T, E>, E> {
        Stream::await(self)
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

pub struct CancelStream<T: Send, E: Send> {
    core: Option<Core<Stream<T, E>>>,
    count: u64,
}

impl<T: Send, E: Send> CancelStream<T, E> {
    fn new(core: Core<Stream<T, E>>, count: u64) -> CancelStream<T, E> {
        CancelStream {
            core: Some(core),
            count: count,
        }
    }

    fn none() -> CancelStream<T, E> {
        CancelStream {
            core: None,
            count: 0,
        }
    }
}

impl<T: Send, E: Send> Cancel<Stream<T, E>> for CancelStream<T, E> {
    fn cancel(self) -> Option<Stream<T, E>> {
        let CancelStream { core, count } = self;

        if !core.is_some() {
            return None;
        }

        if core::get(&core).consumer_ready_cancel(count) {
            return Some(Stream { core: core });
        }

        None
    }
}

pub struct Sender<T: Send, E: Send> {
    core: Option<Core<Stream<T, E>>>,
}

impl<T: Send, E: Send> Sender<T, E> {
    pub fn send(&self, val: T) {
        let rest = Stream { core: self.core.clone() };
        core::get(&self.core).complete(Ok(Some((val, rest))), false);
    }

    pub fn done(self) {
        self.receive(move |res| {
            if let Ok(mut p) = res {
                core::take(&mut p.core).complete(Ok(None), true)
            }
        });
    }

    pub fn fail(mut self, err: E) {
        core::take(&mut self.core).complete(Err(AsyncError::wrap(err)), true);
    }

    pub fn is_ready(&self) -> bool {
        core::get(&self.core).producer_is_ready()
    }

    pub fn is_err(&self) -> bool {
        core::get(&self.core).producer_is_err()
    }

    fn poll(mut self) -> Result<AsyncResult<Sender<T, E>, ()>, Sender<T, E>> {
        debug!("Sender::poll; is_ready={}", self.is_ready());

        let core = core::take(&mut self.core);

        match core.producer_poll() {
            Some(res) => Ok(res),
            None => Err(Sender { core: Some(core) })
        }
    }

    pub fn ready<F: FnOnce(Sender<T, E>) + Send>(mut self, f: F) {
        core::take(&mut self.core).producer_ready(f);
    }

    pub fn await(self) -> AsyncResult<Sender<T, E>, ()> {
        core::get(&self.core).producer_await();
        self.poll().ok().expect("Sender not ready")
    }
}


impl<T: Send, E: Send> Async for Sender<T, E> {
    type Value = Sender<T, E>;
    type Error = ();
    type Cancel = CancelSender;

    fn is_ready(&self) -> bool {
        Sender::is_ready(self)
    }

    fn is_err(&self) -> bool {
        Sender::is_err(self)
    }

    fn poll(self) -> Result<AsyncResult<Sender<T, E>, ()>, Sender<T, E>> {
        Sender::poll(self)
    }

    fn ready<F: FnOnce(Sender<T, E>) + Send>(self, f: F) -> CancelSender {
        Sender::ready(self, f);
        CancelSender
    }
}

impl<T: Send, E: Send> FromCore for Stream<T, E> {
    type Producer = Sender<T, E>;

    fn consumer(core: Core<Stream<T, E>>) -> Stream<T, E> {
        Stream { core: Some(core) }
    }

    fn producer(core: Core<Stream<T, E>>) -> Sender<T, E> {
        Sender { core: Some(core) }
    }
}

impl<T: Send, E: Send> fmt::Debug for Sender<T, E> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Sender<?>")
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for Sender<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            core::take(&mut self.core).complete(Err(AsyncError::canceled()), true);
        }
    }
}

pub struct CancelSender;

impl<T: Send, E: Send> Cancel<Sender<T, E>> for CancelSender {
    fn cancel(self) -> Option<Sender<T, E>> {
        None
    }
}

#[unsafe_no_drop_flag]
pub struct StreamIter<T: Send, E: Send> {
    core: Option<Core<Stream<T, E>>>,
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

pub fn from_core<T: Send, E: Send>(core: Core<Future<Head<T, E>, E>>) -> Stream<T, E> {
    use std::mem;
    Stream { core: Some(unsafe { mem::transmute(core) })}
}
