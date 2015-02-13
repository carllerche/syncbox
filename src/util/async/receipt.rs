use super::Async;
use super::core::Core;
use std::{mem, ptr};

pub struct Receipt<A: Async> {
    core: *const (),
    count: u64,
}

unsafe impl<A: Async> Send for Receipt<A> { }

pub fn new<A: Async, T: Send, E: Send>(core: Core<T, E>, count: u64) -> Receipt<A> {
    Receipt {
        core: unsafe { mem::transmute(core) },
        count: count,
    }
}

pub fn none<A: Async>() -> Receipt<A> {
    Receipt {
        core: ptr::null(),
        count: 0,
    }
}

pub fn parts<A: Async, T: Send, E: Send>(receipt: Receipt<A>) -> (Option<Core<T, E>>, u64) {
    unsafe { mem::transmute((receipt.core, receipt.count)) }
}
