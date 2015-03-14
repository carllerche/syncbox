//! A collection of utilities for writing concurrent code.

#![crate_name = "syncbox"]

// Enable some features
#![feature(unsafe_destructor)]
#![feature(unsafe_no_drop_flag)]

// Embrace edge
#![feature(alloc, core, std_misc)]
#![cfg_attr(test, feature(old_io))]

extern crate alloc;
extern crate core;

#[macro_use]
extern crate log;

pub mod util;
