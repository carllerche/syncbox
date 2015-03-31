//! A collection of utilities for writing concurrent code.

#![crate_name = "syncbox"]

// Embrace edge
#![feature(alloc, core, std_misc, unsafe_destructor)]

extern crate alloc;
extern crate core;

#[macro_use]
extern crate log;

pub use self::array_queue::ArrayQueue;
pub use self::linked_queue::LinkedQueue;
pub use self::thread_pool::ThreadPool;
pub use self::queue::{Queue, SyncQueue};
pub use self::run::Run;

pub mod atomic;
mod array_queue;
mod linked_queue;
mod thread_pool;
mod queue;
mod run;
