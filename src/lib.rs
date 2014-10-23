#![crate_name = "syncbox"]

// Enable some features
#![feature(phase)]
#![feature(unboxed_closures)]
#![feature(overloaded_calls)]
#![feature(unsafe_destructor)]
#![feature(if_let)]
#![feature(while_let)]
#![feature(globs)]

extern crate alloc;
extern crate core;
extern crate libc;
extern crate time;
extern crate sync;

use std::time::Duration;

pub use sync::atomic;
pub use queues::LinkedQueue;
pub use executors::ThreadPool;

mod executors;
mod ffi;
mod queues;

pub mod future;
pub mod locks;

// ===== Various traits =====

pub trait Executor {
    /// Executes the task
    fn execute<F: FnOnce() + Send>(task: F);
}

pub trait LifeCycle {
    /// Transition into a started state
    fn start(&mut self);

    /// Transition into a stopped state
    fn stop(&mut self);
}

pub trait Consume<T> {
    /// Retrieves and removes the head of the queue.
    fn take(&self) -> Option<T> {
        self.take_wait(Duration::milliseconds(0))
    }

    /// Retrieves and removes the head of the queue, waiting if necessary up to
    /// the specified wait time.
    fn take_wait(&self, timeout: Duration) -> Option<T>;
}

pub trait Produce<T> {
    /// Inserts the value into the queue.
    fn put(&self, val: T) -> Result<(), T> {
        self.put_wait(val, Duration::milliseconds(0))
    }

    /// Inserts the value into the queue, waiting if necessary up to the
    /// specified wait time.
    fn put_wait(&self, val: T, timeout: Duration) -> Result<(), T>;
}
