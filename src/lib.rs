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

pub use sync::atomic;
pub use queue::{Consume, Produce};
pub use linked_queue::LinkedQueue;

mod queue;
mod linked_queue;

pub mod future;
pub mod locks;
