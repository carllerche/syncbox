#![crate_name = "syncbox"]

// Enable some features
#![feature(phase)]
#![feature(globs)]
#![feature(unboxed_closures)]
#![feature(unsafe_destructor)]

extern crate alloc;
extern crate core;
extern crate libc;

pub mod util;
