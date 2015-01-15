//! A collection of utilities for writing concurrent code.

#![crate_name = "syncbox"]

// Enable some features
#![feature(int_uint)]
#![feature(unboxed_closures)]
#![feature(unsafe_destructor)]

// Embrace edge
#![allow(unstable)]

extern crate alloc;

#[macro_use]
extern crate log;

pub mod util;
