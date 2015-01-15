use super::{Async, Future, AsyncResult, AsyncError};
use super::core::{Core, OptionCore, FromCore};
use std::fmt;

pub type Head<T, E> = Option<(T, Stream<T, E>)>;

#[unsafe_no_drop_flag]
pub struct Stream<T: Send, E: Send> {
    core: OptionCore<Head<T, E>, E, Produce<T, E>>,
}

impl<T: Send, E: Send> Stream<T, E> {
    pub fn pair() -> (Stream<T, E>, Produce<T, E>) {
        let core = Core::new();
        let stream = Stream { core: OptionCore::new(core.clone()) };

        (stream, Produce { core: OptionCore::new(core) })
    }

    pub fn receive<F: FnOnce(AsyncResult<Head<T, E>, E>) + Send>(mut self, f: F) {
        self.core.take().consumer_receive(f);
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

    pub fn take(self, _n: uint) -> Stream<T, E> {
        unimplemented!();
    }

    pub fn take_while<F: Fn(&T) -> bool + Send>(self, _f: F) -> Stream<T, E> {
        unimplemented!();
    }
}

impl<T: Send, E: Send> Async<Head<T, E>, E> for Stream<T, E> {
    fn receive<F: FnOnce(AsyncResult<Head<T, E>, E>) + Send>(self, f: F) {
        Stream::receive(self, f);
    }
}

impl<T: Send, E: Send> fmt::Show for Stream<T, E> {
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

pub struct Produce<T: Send, E: Send> {
    core: OptionCore<Head<T, E>, E, Produce<T, E>>,
}

impl<T: Send, E: Send> Produce<T, E> {
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

    pub fn receive<F: FnOnce(AsyncResult<Produce<T, E>, ()>) + Send>(mut self, f: F) {
        self.core.take().producer_receive(f);
    }
}

impl<T: Send, E: Send> FromCore<Head<T, E>, E> for Produce<T, E> {
    fn from_core(core: Core<Head<T, E>, E, Produce<T, E>>) -> Produce<T, E> {
        Produce { core: OptionCore::new(core) }
    }
}

impl<T: Send, E: Send> fmt::Show for Produce<T, E> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "Produce<?>")
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send> Drop for Produce<T, E> {
    fn drop(&mut self) {
        if self.core.is_some() {
            self.core.take().complete(Err(AsyncError::canceled()), true);
        }
    }
}

#[unsafe_no_drop_flag]
pub struct StreamIter<T: Send, E: Send> {
    core: OptionCore<Head<T, E>, E, Produce<T, E>>,
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

pub fn from_core<T: Send, E: Send, P: FromCore<Head<T, E>, E>>(core: Core<Head<T, E>, E, P>) -> Stream<T, E> {
    use std::mem;
    Stream { core: OptionCore::new(unsafe { mem::transmute(core) })}
}
