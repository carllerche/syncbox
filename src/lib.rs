#![crate_name = "syncbox"]

// Enable some features
#![feature(phase)]
#![feature(globs)]
#![feature(unboxed_closures)]
#![feature(unsafe_destructor)]

extern crate alloc;
extern crate core;
extern crate libc;

pub use queues::LinkedQueue;
// pub use executors::ThreadPool;

// mod executors;
mod queues;

pub mod util;

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
