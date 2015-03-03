use super::Async;
use super::core::Core;
use std::{mem, ptr};
use std::marker::PhantomData;

pub struct Receipt<A> {
    core: *const (),
    count: u64,
    marker: PhantomData<A>,
}

unsafe impl<A: Async> Send for Receipt<A> { }

pub fn new<A: Async, T: Send, E: Send>(core: Core<T, E>, count: u64) -> Receipt<A> {
    Receipt {
        core: unsafe { mem::transmute(core) },
        count: count,
        marker: PhantomData,
    }
}

pub fn none<A: Async>() -> Receipt<A> {
    Receipt {
        core: ptr::null(),
        count: 0,
        marker: PhantomData,
    }
}

pub fn parts<A: Async, T: Send, E: Send>(receipt: Receipt<A>) -> (Option<Core<T, E>>, u64) {
    unsafe { mem::transmute((receipt.core, receipt.count)) }
}
