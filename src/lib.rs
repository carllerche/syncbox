//! A collection of utilities for writing concurrent code.

#![crate_name = "syncbox"]

// Embrace edge
#![feature(alloc, core, std_misc, unsafe_destructor)]

extern crate alloc;
extern crate core;

#[macro_use]
extern crate log;

pub mod util;
