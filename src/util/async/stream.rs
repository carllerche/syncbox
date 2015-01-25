use super::{Async, Future, Cancel, AsyncResult, AsyncError};
use super::core::{Core, OptionCore, FromCore};
use std::fmt;

pub type Head<T, E> = Option<(T, Stream<T, E>)>;

#[unsafe_no_drop_flag]
pub struct Stream<T: Send, E: Send> {
    core: OptionCore<Stream<T, E>>,
}

impl<T: Send, E: Send> Stream<T, E> {
    pub fn pair() -> (Stream<T, E>, Generate<T, E>) {
        let core = Core::new();
        let stream = Stream { core: OptionCore::new(core.clone()) };

        (stream, Generate { core: OptionCore::new(core) })
    }

    pub fn is_ready(&self) -> bool {
        self.core.get().consumer_is_ready()
    }

    pub fn poll(mut self) -> Result<AsyncResult<Head<T, E>, E>, Stream<T, E>> {
        let core = self.core.take();

        match core.consumer_poll() {
            Some(res) => Ok(res),
            None => Err(Stream { core: OptionCore::new(core) })
        }
    }

    pub fn ready<F: FnOnce(Stream<T, E>) + Send>(mut self, f: F) -> CancelStream<T, E> {
        let core = self.core.take();

        match core.consumer_ready(f) {
            Some(count) => CancelStream::new(core, count),
            None => CancelStream::none(),
        }
    }

    pub fn await(mut self) -> AsyncResult<Head<T, E>, E> {
        self.core.take().consumer_await()
    }

    pub fn iter(mut self) -> StreamIter<T, E> {
        StreamIter { core: OptionCore::new(self.core.take()) }
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

    // TODO: Figure out what to do when the condition errors
    pub fn take_until<A>(self, _cond: A) -> Stream<T, E>
            where A: Async {
        unimplemented!();
    }
}

impl<T: Send, E: Send> Async for Stream<T, E> {
    type Value = Head<T, E>;
    type Error = E;
    type Cancel = CancelStream<T, E>;

    fn is_ready(&self) -> bool {
        Stream::is_ready(self)
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
            self.core.take().cancel();
        }
    }
}

pub struct CancelStream<T: Send, E: Send> {
    core: OptionCore<Stream<T, E>>,
    count: u64,
}

impl<T: Send, E: Send> CancelStream<T, E> {
    fn new(core: Core<Stream<T, E>>, count: u64) -> CancelStream<T, E> {
        CancelStream {
            core: OptionCore::new(core),
            count: count,
        }
    }

    fn none() -> CancelStream<T, E> {
        CancelStream {
            core: OptionCore::none(),
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

        if core.get().consumer_ready_cancel(count) {
            return Some(Stream { core: core });
        }

        None
    }
}

pub struct Generate<T: Send, E: Send> {
    core: OptionCore<Stream<T, E>>,
}

impl<T: Send, E: Send> Generate<T, E> {
    pub fn send(&self, val: T) {
        let rest = Stream { core: self.core.clone() };
        self.core.get().complete(Ok(Some((val, rest))), false);
    }

    pub fn done(self) {
        self.receive(move |res| {
            if let Ok(mut p) = res {
                p.core.take().complete(Ok(None), true)
            }
        });
    }

    pub fn fail(mut self, err: E) {
        self.core.take().complete(Err(AsyncError::wrap(err)), true);
    }

    pub fn is_ready(&self) -> bool {
        self.core.get().producer_is_ready()
    }

    fn poll(mut self) -> Result<AsyncResult<Generate<T, E>, ()>, Generate<T, E>> {
        debug!("Generate::poll; is_ready={}", self.is_ready());

        let core = self.core.take();

        match core.producer_poll() {
            Some(res) => Ok(res),
            None => Err(Generate { core: OptionCore::new(core) })
        }
    }

    pub fn ready<F: FnOnce(Generate<T, E>) + Send>(mut self, f: F) {
        self.core.take().producer_ready(f);
    }

    pub fn await(self) -> AsyncResult<Generate<T, E>, ()> {
        self.core.get().producer_await();
        self.poll().ok().expect("Generate not ready")
    }
}


impl<T: Send, E: Send> Async for Generate<T, E> {
    type Value = Generate<T, E>;
    type Error = ();
    type Cancel = CancelGenerate;

    fn is_ready(&self) -> bool {
        Generate::is_ready(self)
    }

    fn poll(self) -> Result<AsyncResult<Generate<T, E>, ()>, Generate<T, E>> {
        Generate::poll(self)
    }

    fn ready<F: FnOnce(Generate<T, E>) + Send>(self, f: F) -> CancelGenerate {
        Generate::ready(self, f);
        CancelGenerate
    }
}

impl<T: Send, E: Send> FromCore for Stream<T, E> {
    type Producer = Generate<T, E>;

    fn consumer(core: Core<Stream<T, E>>) -> Stream<T, E> {
        Stream { core: OptionCore::new(core) }
    }

    fn producer(core: Core<Stream<T, E>>) -> Generate<T, E> {
        Generate { core: OptionCore::new(core) }
    }
}

impl<T: Send, E: Send> fmt::Debug for Generate<T, E> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Generate<?>")
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for Generate<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            self.core.take().complete(Err(AsyncError::canceled()), true);
        }
    }
}

pub struct CancelGenerate;

impl<T, E> Cancel<Generate<T, E>> for CancelGenerate {
    fn cancel(self) -> Option<Generate<T, E>> {
        None
    }
}

#[unsafe_no_drop_flag]
pub struct StreamIter<T: Send, E: Send> {
    core: OptionCore<Stream<T, E>>,
}

impl<T: Send, E: Send> Iterator for StreamIter<T, E> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        use std::mem;

        match self.core.get().consumer_await() {
            Ok(Some((h, mut rest))) => {
                mem::replace(&mut self.core, OptionCore::new(rest.core.take()));
                Some(h)
            }
            Ok(None) => {
                let _ = self.core.take();
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
            self.core.take().cancel();
        }
    }
}

pub fn from_core<T: Send, E: Send>(core: Core<Future<Head<T, E>, E>>) -> Stream<T, E> {
    use std::mem;
    Stream { core: OptionCore::new(unsafe { mem::transmute(core) })}
}
