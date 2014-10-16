#![crate_name = "syncbox"]

// Enable some features
#![feature(phase)]
#![feature(unboxed_closures)]
#![feature(overloaded_calls)]
#![feature(unsafe_destructor)]
#![feature(if_let)]
#![feature(globs)]

extern crate alloc;
extern crate core;
extern crate libc;
extern crate time;
extern crate sync;

pub use sync::atomic;

pub mod future;
pub mod locks;
