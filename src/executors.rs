use std::time::Duration;
use atomic::AtomicUint;
use {Executor, LifeCycle, Consume};

// WIP

pub struct ThreadPool<'a, T> {
    target_size: uint,
    max_size: uint,
    keepalive: Duration,
    queue: Box<Consume<T> +'a>,
}

impl<'a, T> Executor for ThreadPool<'a T> {
    fn execute<F: FnOnce() + Send>(task: F) {
        unimplemented!()
    }
}
