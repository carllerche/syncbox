#[allow(dead_code)]

use self::Lifecycle::*;
use super::{Async, BoxedReceive, AsyncResult, AsyncError};
use util::atomic::{self, AtomicU64, AtomicUsize, Ordering};
use std::{fmt, mem, ptr};
use std::num::FromPrimitive;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::thread::Thread;
use alloc::heap;

/*
 *
 * ===== Core =====
 *
 */

// Core implementation of Future & Stream
#[unsafe_no_drop_flag]
pub struct Core<A: Async + FromCore> {
    ptr: *mut CoreInner<A>,
}

impl<A: Async + FromCore> Core<A> {
    pub fn new() -> Core<A> {
        let ptr = Box::new(CoreInner::<A>::new());
        Core { ptr: unsafe { mem::transmute(ptr) }}
    }

    pub fn with_value(val: AsyncResult<A::Value, A::Error>) -> Core<A> {
        let ptr = Box::new(CoreInner::<A>::with_value(val));
        Core { ptr: unsafe { mem::transmute(ptr) }}
    }

    /// Returns true if the calling `consumer_poll` will return a value.
    pub fn consumer_is_ready(&self) -> bool {
        self.inner().consumer_is_ready()
    }

    pub fn consumer_is_err(&self) -> bool {
        self.inner().consumer_is_err()
    }

    /// Returns the underlying value if it has been realized, None otherwise.
    pub fn consumer_poll(&self) -> Option<AsyncResult<A::Value, A::Error>> {
        self.inner().consumer_poll()
    }

    /// Blocks the thread until calling `consumer_poll` will return a value.
    pub fn consumer_await(&self) -> AsyncResult<A::Value, A::Error> {
        debug!("Core::consumer_await");

        // Ensure not already consuming
        if self.inner().state.load(Relaxed).is_invoking_consumer() {
            panic!("cannot block thread when in a callback");
        }

        let th = Thread::current();
        self.inner().consumer_ready(move |_| th.unpark());

        while !self.consumer_is_ready() {
            Thread::park();
        }

        self.consumer_poll().expect("result not ready")
    }

    /// Registers a callback that will be invoked when calling `consumer_poll`
    /// will return a value.
    pub fn consumer_ready<F: FnOnce(A) + Send>(&self, f: F) -> Option<u64> {
        self.inner().consumer_ready(move |core| f(FromCore::consumer(core)))
    }

    pub fn consumer_ready_cancel(&self, count: u64) -> bool {
        self.inner().consumer_ready_cancel(count)
    }

    pub fn producer_is_ready(&self) -> bool {
        self.inner().producer_is_ready()
    }

    pub fn producer_is_err(&self) -> bool {
        self.inner().producer_is_err()
    }

    pub fn producer_poll(&self) -> Option<AsyncResult<A::Producer, ()>> {
        self.inner().producer_poll()
    }

    pub fn producer_await(&self) {
        debug!("Core::producer_await");

        // Ensure not already producing
        if self.inner().state.load(Relaxed).is_invoking_producer() {
            panic!("cannot block thread when in a callback");
        }

        let th = Thread::current();

        self.inner().producer_ready(move |_| th.unpark());

        while !self.producer_is_ready() {
            Thread::park();
        }
    }

    pub fn producer_ready<F: FnOnce(A::Producer) + Send>(&self, f: F) {
        self.inner().producer_ready(move |core| f(FromCore::producer(core)));
    }

    pub fn complete(&self, val: AsyncResult<A::Value, A::Error>, last: bool) {
        self.inner().complete(val, last);
    }

    pub fn cancel(&self) {
        self.inner().cancel();
    }

    #[inline]
    fn inner(&self) -> &CoreInner<A> {
        unsafe { &*self.ptr }
    }

    #[inline]
    fn inner_mut(&mut self) -> &mut CoreInner<A> {
        unsafe { &mut *self.ptr }
    }

    fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
}

impl<A: Async + FromCore> Clone for Core<A> {
    fn clone(&self) -> Core<A> {
        // Increments ref count and returns a new core
        self.inner().core()
    }
}

#[unsafe_destructor]
impl<A: Async + FromCore> Drop for Core<A> {
    fn drop(&mut self) {
        // Handle memory already zeroed out
        if self.ptr.is_null() { return; }

        if self.inner().ref_dec(Release) != 1 {
            return;
        }

        // This fence is needed to prevent reordering of use of the data and deletion of the data.
        // Because it is marked `Release`, the decreasing of the reference count synchronizes with
        // this `Acquire` fence. This means that use of the data happens before decreasing the
        // reference count, which happens before this fence, which happens before the deletion of
        // the data.
        //
        // As explained in the [Boost documentation][1],
        //
        // > It is important to enforce any possible access to the object in one thread (through an
        // > existing reference) to *happen before* deleting the object in a different thread. This
        // > is achieved by a "release" operation after dropping a reference (any access to the
        // > object through this reference must obviously happened before), and an "acquire"
        // > operation before deleting the object.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        atomic::fence(Acquire);

        unsafe {
            let curr = self.inner().state.load(Relaxed);

            if curr.is_ready() {
                // The future currently contains the realized value, free it.
                let _ = self.inner().take_val();
            }

            let _ = self.inner_mut().consumer_wait.take();
            let _ = self.inner_mut().producer_wait.take();

            heap::deallocate(
                self.ptr as *mut u8,
                mem::size_of::<CoreInner<A>>(),
                mem::min_align_of::<CoreInner<A>>());
        }
    }
}

unsafe impl<A: Async + FromCore> Send for Core<A> {
}

/*
 *
 * ===== OptionCore =====
 *
 */

pub struct OptionCore<A: Async + FromCore> {
    core: Core<A>,
}

impl<A: Async + FromCore> OptionCore<A> {
    #[inline]
    pub fn new(core: Core<A>) -> OptionCore<A> {
        OptionCore { core: core }
    }

    pub fn none() -> OptionCore<A> {
        let core = unsafe { mem::transmute(0u64) };
        OptionCore { core: core }
    }

    #[inline]
    pub fn is_some(&self) -> bool {
        !self.core.is_null()
    }

    pub fn get(&self) -> &Core<A> {
        assert!(self.is_some(), "Core has already been consumed");
        &self.core
    }

    #[inline]
    pub fn take(&mut self) -> Core<A> {
        assert!(self.is_some(), "Core has already been consumed");
        unsafe { mem::replace(&mut self.core, mem::zeroed()) }
    }
}

impl<A: Async + FromCore> Clone for OptionCore<A> {
    fn clone(&self) -> OptionCore<A> {
        OptionCore { core: self.core.clone() }
    }
}

#[unsafe_destructor]
impl<A: Async + FromCore> Drop for OptionCore<A> {
    fn drop(&mut self) {
        if self.is_some() {
            let _ = self.take();
        }
    }
}

/*
 *
 * ===== FromCore =====
 *
 */

pub trait FromCore {
    type Producer: Send;

    fn consumer(core: Core<Self>) -> Self;

    fn producer(core: Core<Self>) -> Self::Producer;
}

/*
 *
 * ===== CoreInner<T> =====
 *
 */


struct CoreInner<A: Async + FromCore> {
    refs: AtomicUsize,
    state: AtomicState,
    consumer_wait: Option<Callback<A>>,
    producer_wait: Option<Callback<A>>,
    val: AsyncResult<A::Value, A::Error>, // May be uninitialized
}

impl<A: Async + FromCore> CoreInner<A> {
    fn new() -> CoreInner<A> {
        CoreInner {
            refs: AtomicUsize::new(1),
            state: AtomicState::new(),
            consumer_wait: None,
            producer_wait: None,
            val: unsafe { mem::uninitialized() },
        }
    }

    fn with_value(val: AsyncResult<A::Value, A::Error>) -> CoreInner<A> {
        CoreInner {
            refs: AtomicUsize::new(1),
            state: AtomicState::of(Ready),
            consumer_wait: None,
            producer_wait: None,
            val: val,
        }
    }

    pub fn consumer_is_ready(&self) -> bool {
        self.state.load(Relaxed).is_ready()
    }

    pub fn consumer_is_err(&self) -> bool {
        if !self.state.load(Acquire).is_ready() {
            return false;
        }

        self.val.is_err()
    }

    pub fn consumer_poll(&self) -> Option<AsyncResult<A::Value, A::Error>> {
        let curr = self.state.load(Relaxed);

        debug!("Core::consumer_poll; state={:?}", curr);

        if !curr.is_ready() {
            return None;
        }

        Some(self.consume_val(curr))
    }

    fn consumer_ready<F: FnOnce(Core<A>) + Send>(&self, f: F) -> Option<u64> {
        let mut curr = self.state.load(Relaxed);

        debug!("Core::consumer_ready; state={:?}", curr);

        // If the future is already complete, then there is no need to move the
        // callback to a Box. Consume the future result directly.
        if curr.is_ready() && !curr.is_invoking_consumer() {
            // Transition the state to New / ProducerWait
            self.state.invoking_consumer_ready();

            debug!("  - Invoking consumer");
            f(self.core());

            curr = self.state.done_invoking_consumer_ready();

            if curr.is_consumer_notify() {
                self.notify_consumer_loop(curr);
            }

            return None;
        }

        // At this point, odds are that the callback will happen async. Move
        // the callback to a box.
        self.put_consumer_wait(Box::new(f));

        // Execute wait strategy
        self.consumer_wait(curr)
    }

    // Transition the state to indicate that a consumer is waiting.
    //
    fn consumer_wait(&self, mut curr: State) -> Option<u64> {
        let mut next;
        let mut notify_producer;

        debug!("Core::consumer_wait; state={:?}", curr);

        loop {
            notify_producer = false;

            next = match curr.lifecycle() {
                New => {
                    // The future is in a fresh state (neither consumer or
                    // producer is waiting). Indicate that the consumer is
                    // waiting.
                    curr.with_lifecycle(ConsumerWait)
                }
                ProducerWait => {
                    if curr.is_invoking_producer() {
                        curr.with_lifecycle(ProducerNotify)
                    } else {
                        notify_producer = true;
                        curr.with_lifecycle(ConsumerWait)
                    }
                }
                Ready | ReadyProducerWait => {
                    debug!("  - completing consumer");

                    // Handles the recursion case
                    self.notify_consumer(curr);
                    return None;
                }
                Canceled | ConsumerWait | ConsumerNotify | ProducerNotify | ProducerNotifyCanceled => {
                    panic!("invalid state {:?}", curr.lifecycle())
                }
            };

            // Increment the callback count
            next = next.inc_count();

            let actual = self.state.compare_and_swap(curr, next, Release);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                break;
            }

            curr = actual;
        }

        if notify_producer {
            debug!("  - notifying producer");
            // Use a fence to acquire the producer callback
            atomic::fence(Acquire);

            // Notify the producer
            next = self.notify_producer(next);
        }

        Some(next.count())
    }

    fn notify_consumer(&self, curr: State) {
        // Already in a consumer callback, track that it should be invoked
        // again
        if curr.is_invoking_consumer() {
            debug!("  - already consuming, defer");
            return self.defer_consumer_notify(curr);
        }

        self.notify_consumer_loop(curr)
    }

    fn defer_consumer_notify(&self, mut curr: State) {
        loop {
            let next = match curr.lifecycle() {
                Ready => curr.with_lifecycle(ConsumerNotify),
                _ => panic!("invalid state {:?}", curr.lifecycle()),
            };

            let actual = self.state.compare_and_swap(curr, next, Relaxed);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                return;
            }

            curr = actual;
        }
    }

    fn notify_consumer_loop(&self, mut curr: State) {
        loop {
            let cb = self.take_consumer_wait();

            self.state.invoking_consumer_ready();

            // Invoke the callback
            debug!("  - notifying consumer");
            cb.receive_boxed(self.core());
            debug!("  - consumer notified");

            curr = self.state.done_invoking_consumer_ready();

            if curr.is_consumer_notify() {
                continue;
            }

            return;
        }
    }

    fn consumer_ready_cancel(&self, count: u64) -> bool {
        let curr = self.state.load(Relaxed);

        debug!("Core::consumer_ready_cancel; count={}; state={:?}", count, curr);

        loop {
            let next = match curr.lifecycle() {
                ConsumerWait | ProducerNotify => {
                    if count != curr.count() {
                        // Counts don't match, can't cancel the callback
                        return false;
                    }

                    assert!(!curr.is_invoking_consumer());

                    curr.with_lifecycle(New)
                }
                New | ProducerWait | Ready | ReadyProducerWait | ProducerNotifyCanceled | ConsumerNotify | Canceled => {
                    // No pending consumer callback to cancel
                    return false;
                }
            };

            let actual = self.state.compare_and_swap(curr, next, Relaxed);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                return true;
            }
        }
    }

    fn producer_is_ready(&self) -> bool {
        let curr = self.state.load(Relaxed);
        curr.is_producer_ready()
    }

    fn producer_is_err(&self) -> bool {
        unimplemented!();
    }

    pub fn producer_poll(&self) -> Option<AsyncResult<A::Producer, ()>> {
        let curr = self.state.load(Relaxed);

        debug!("Core::producer_poll; state={:?}", curr);

        if !curr.is_producer_ready() {
            return None;
        }

        if curr.is_canceled() {
            return Some(Err(AsyncError::canceled()));
        }

        Some(Ok(FromCore::producer(self.core())))
    }

    fn producer_ready<F: FnOnce(Core<A>) + Send>(&self, f: F) {
        let mut curr = self.state.load(Relaxed);

        debug!("Core::producer_ready; state={:?}", curr);

        // Don't recurse produce callbacks
        if !curr.is_invoking_producer() {
            if curr.is_consumer_wait() || curr.is_canceled() {
                // The producer callback is about to be invoked
                self.state.invoking_producer_ready(curr);

                debug!("  - Invoking producer");
                f(self.core());

                curr = self.state.done_invoking_producer_ready();

                if curr.is_producer_notify() {
                    self.notify_producer_loop(curr);
                }

                return;
            }
        }

        // At this point, odds are that the callback will happen async. Move
        // the callback to a box.
        self.put_producer_wait(Box::new(f));

        // Do producer
        self.producer_wait(curr);
    }

    fn producer_wait(&self, mut curr: State) -> State {
        loop {
            let next = match curr.lifecycle() {
                New => {
                    // Notify the producer when the consumer registers interest
                    curr.with_lifecycle(ProducerWait)
                }
                ConsumerWait => {
                    debug!("  - notifying producer");
                    // Notify producer now
                    self.notify_producer(curr);
                    return curr;
                }
                Canceled => {
                    debug!("  - notifying producer");
                    // Notify producer now
                    self.notify_producer(curr);
                    return curr;
                }
                Ready => {
                    // Track that there is a pending value as well as producer
                    // interest
                    curr.with_lifecycle(ReadyProducerWait)
                }
                ProducerWait | ReadyProducerWait | ProducerNotify | ProducerNotifyCanceled | ConsumerNotify => {
                    panic!("invalid state {:?}", curr.lifecycle())
                }
            };

            let actual = self.state.compare_and_swap(curr, next, Release);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", actual, next);
                return next;
            }

            curr = actual;
        }
    }

    fn cancel(&self) {
        let mut curr = self.state.load(Relaxed);
        let mut next;
        let mut read_val;
        let mut notify_producer;

        debug!("Core::cancel; state={:?}", curr);

        loop {
            read_val = false;
            notify_producer = false;

            next = match curr.lifecycle() {
                New => {
                    curr.with_lifecycle(Canceled)
                }
                ProducerWait => {
                    if curr.is_invoking_producer() {
                        curr.with_lifecycle(ProducerNotifyCanceled)
                    } else {
                        notify_producer = true;
                        curr.with_lifecycle(Canceled)
                    }
                }
                Ready => {
                    debug!("   ~~~ WARN!! Transitioning from Ready -> Cancel ~~~");
                    read_val = true;
                    curr.with_lifecycle(Canceled)
                }
                ReadyProducerWait => {
                    debug!("   ~~~ WARN!! Transitioning from Ready -> Cancel ~~~");
                    read_val = true;
                    notify_producer = true;
                    curr.with_lifecycle(Canceled)
                }
                Canceled | ConsumerWait | ConsumerNotify | ProducerNotify | ProducerNotifyCanceled => {
                    panic!("invalid state {:?}", curr.lifecycle())
                }
            };

            let actual = self.state.compare_and_swap(curr, next, Release);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                break;
            }

            curr = actual;
        }

        if read_val || notify_producer {
            atomic::fence(Acquire);
        }

        if read_val {
            let _ = unsafe { self.take_val() };
        }

        if notify_producer {
            // Notify the producer
            self.notify_producer(next);
        }
    }

    fn complete(&self, val: AsyncResult<A::Value, A::Error>, last: bool) {
        let mut curr = self.state.load(Relaxed);
        let mut next;

        debug!("Core::complete; state={:?}; success={}; last={:?}", curr, val.is_ok(), last);

        // Do nothing if canceled
        if curr.is_canceled() {
            return;
        }

        // Set the val
        unsafe { self.put_val(val); }

        loop {
            next = match curr.lifecycle() {
                New => {
                    curr.with_lifecycle(Ready)
                }
                Canceled => {
                    debug!("  - dropping val");
                    // The value was set, it will not get freed on drop, so
                    // free it now.
                    let _ = unsafe { self.take_val() };
                    return;
                }
                ConsumerWait => {
                    curr.with_lifecycle(Ready)
                }
                ProducerWait | ProducerNotify | ProducerNotifyCanceled | ConsumerNotify | Ready | ReadyProducerWait => {
                    panic!("invalid state {:?}", curr.lifecycle())
                }
            };

            let actual = self.state.compare_and_swap(curr, next, Release);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", actual, next);
                break;
            }

            curr = actual;
        }

        if curr.is_consumer_wait() && next.is_ready() {
            // Use a fence to acquire the consumer callback
            atomic::fence(Acquire);

            // Notify the consumer that the value is ready
            self.notify_consumer(next);
        }
    }

    fn notify_producer(&self, curr: State) -> State {
        debug!("Core::notify_producer");

        if curr.is_invoking_producer() {
            return self.defer_producer_notify(curr);
        }

        self.notify_producer_loop(curr)
    }

    // Track that the producer callback would have been invoked had their not
    // been a producer callback currently being executed
    fn defer_producer_notify(&self, mut curr: State) -> State {
        loop {
            let next = match curr.lifecycle() {
                ConsumerWait => curr.with_lifecycle(ProducerNotify),
                Canceled => curr.with_lifecycle(ProducerNotifyCanceled),
                _ => panic!("invalid state {:?}", curr),
            };

            let actual = self.state.compare_and_swap(curr, next, Relaxed);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                return next;
            }

            curr = actual;
        }
    }

    // When a produce callback is being invoked, the future does not permit
    // calling into the next produce callback until the first one has returned.
    // This requires invoking produce callbacks in a loop, and calling the next
    // one as long as the state is ProducerNotify
    fn notify_producer_loop(&self, mut curr: State) -> State {
        loop {
            let cb = self.take_producer_wait();

            // Transition the state to track that a producer callback
            // is being invoked
            curr = self.state.invoking_producer_ready(curr);

            debug!("  - Invoking producer; state={:?}", curr);

            // Invoke the callback
            cb.receive_boxed(self.core());

            // Track that the callback is done being invoked
            curr = self.state.done_invoking_producer_ready();

            debug!("  - Producer invoked; state={:?}", curr);

            if curr.is_producer_notify() {
                continue;
            }

            return curr;
        }
    }

    fn consume_val(&self, mut curr: State) -> AsyncResult<A::Value, A::Error> {
        // Ensure that the memory is synced
        atomic::fence(Acquire);

        // Get the value
        let ret = unsafe { self.take_val() };

        loop {
            let next = match curr.lifecycle() {
                Ready | ConsumerNotify => {
                    curr.with_lifecycle(New)
                }
                ReadyProducerWait => {
                    curr.with_lifecycle(ProducerWait)
                }
                _ => panic!("unexpected state {:?}", curr),
            };

            let actual = self.state.compare_and_swap(curr, next, Relaxed);

            if curr == actual {
                debug!("  - transitioned from {:?} to {:?} (consuming value)", curr, next);
                return ret;
            }

            curr = actual
        }
    }

    unsafe fn put_val(&self, val: AsyncResult<A::Value, A::Error>) {
        ptr::write(mem::transmute(&self.val), val);
    }

    unsafe fn take_val(&self) -> AsyncResult<A::Value, A::Error> {
        ptr::read(&self.val)
    }

    fn put_consumer_wait(&self, cb: Callback<A>) {
        unsafe {
            let s: &mut CoreInner<A> = mem::transmute(self);
            s.consumer_wait = Some(cb);
        }
    }

    fn take_consumer_wait(&self) -> Callback<A> {
        unsafe {
            let s: &mut CoreInner<A> = mem::transmute(self);
            s.consumer_wait.take().expect("consumer_wait is none")
        }
    }

    fn put_producer_wait(&self, cb: Callback<A>) {
        unsafe {
            let s: &mut CoreInner<A> = mem::transmute(self);
            s.producer_wait = Some(cb);
        }
    }

    fn take_producer_wait(&self) -> Callback<A> {
        unsafe {
            let s: &mut CoreInner<A> = mem::transmute(self);
            s.producer_wait.take().expect("producer_wait is none")
        }
    }

    fn ref_inc(&self, order: Ordering) -> usize {
        self.refs.fetch_add(1, order)
    }

    fn ref_dec(&self, order: Ordering) -> usize {
        self.refs.fetch_sub(1, order)
    }

    fn core(&self) -> Core<A> {
        // Using a relaxed ordering is alright here, as knowledge of the original reference
        // prevents other threads from erroneously deleting the object.
        //
        // As explained in the [Boost documentation][1], Increasing the reference counter can
        // always be done with memory_order_relaxed: New references to an object can only be formed
        // from an existing reference, and passing an existing reference from one thread to another
        // must already provide any required synchronization.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        self.ref_inc(Relaxed);
        Core { ptr: unsafe { mem::transmute(self) } }
    }
}

struct AtomicState {
    atomic: AtomicU64,
}

impl AtomicState {
    fn new() -> AtomicState {
        let initial = State::new();
        AtomicState { atomic: AtomicU64::new(initial.as_u64()) }
    }

    fn of(lifecycle: Lifecycle) -> AtomicState {
        let initial = State::new().with_lifecycle(lifecycle);
        AtomicState { atomic: AtomicU64::new(initial.as_u64()) }
    }

    fn load(&self, order: Ordering) -> State {
        let val = self.atomic.load(order);
        State::load(val)
    }

    fn compare_and_swap(&self, old: State, new: State, order: Ordering) -> State {
        let ret = self.atomic.compare_and_swap(old.as_u64(), new.as_u64(), order);
        State::load(ret)
    }

    fn invoking_consumer_ready(&self) {
        self.atomic.fetch_add(CONSUMING_MASK, Relaxed);
    }

    fn done_invoking_consumer_ready(&self) -> State {
        let val = self.atomic.fetch_sub(CONSUMING_MASK, Relaxed);
        State { val: val - CONSUMING_MASK }
    }

    fn invoking_producer_ready(&self, mut curr: State) -> State {
        loop {
            let next = match curr.lifecycle() {
                ConsumerWait | ProducerNotify => {
                    curr.with_lifecycle(ConsumerWait).with_producing()
                }
                Canceled => {
                    curr.with_producing()
                }
                _ => panic!("unexpected state {:?}", curr),
            };

            let actual = self.compare_and_swap(curr, next, Relaxed);

            if curr == actual {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                return next;
            }

            curr = actual
        }
    }

    fn done_invoking_producer_ready(&self) -> State {
        let val = self.atomic.fetch_sub(PRODUCING_MASK, Relaxed);
        State { val: val - PRODUCING_MASK }
    }
}

#[derive(Copy, Eq, PartialEq)]
struct State {
    val: u64,
}

const LIFECYCLE_MASK: u64 = 15;
const CONSUMING_MASK: u64 = 1 << 4;
const PRODUCING_MASK: u64 = 1 << 5;
const COUNT_OFFSET:   u64 = 6;

impl State {
    fn new() -> State {
        State { val: 0 }
    }

    fn load(val: u64) -> State {
        State { val: val }
    }

    fn lifecycle(&self) -> Lifecycle {
        FromPrimitive::from_u64(self.val & LIFECYCLE_MASK)
            .expect("unexpected state value")
    }

    fn with_lifecycle(&self, val: Lifecycle) -> State {
        let val = self.val & !LIFECYCLE_MASK | val as u64;
        State { val: val }
    }

    fn with_producing(&self) -> State {
        State { val: self.val | PRODUCING_MASK }
    }

    fn is_invoking_consumer(&self) -> bool {
        self.val & CONSUMING_MASK == CONSUMING_MASK
    }

    fn is_invoking_producer(&self) -> bool {
        self.val & PRODUCING_MASK == PRODUCING_MASK
    }

    fn is_consumer_wait(&self) -> bool {
        match self.lifecycle() {
            ConsumerWait => true,
            _ => false,
        }
    }

    fn is_ready(&self) -> bool {
        match self.lifecycle() {
            Ready | ReadyProducerWait => true,
            _ => false,
        }
    }

    fn is_producer_ready(&self) -> bool {
        self.is_consumer_wait() || self.is_ready() || self.is_canceled()
    }

    fn is_canceled(&self) -> bool {
        match self.lifecycle() {
            Canceled => true,
            _ => false,
        }
    }

    fn is_consumer_notify(&self) -> bool {
        match self.lifecycle() {
            ConsumerNotify => true,
            _ => false,
        }
    }

    fn is_producer_notify(&self) -> bool {
        match self.lifecycle() {
            ProducerNotify => true,
            _ => false,
        }
    }

    fn count(&self) -> u64 {
        self.val >> COUNT_OFFSET
    }

    fn inc_count(&self) -> State {
        State { val: self.val + (1 << COUNT_OFFSET) }
    }

    fn as_u64(self) -> u64 {
        self.val
    }
}

impl fmt::Debug for State {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "State[count={}; consuming={}; producing={}; lifecycle={:?}]",
               self.count(), self.is_invoking_consumer(), self.is_invoking_producer(), self.lifecycle())
    }
}

type Callback<A> = Box<BoxedReceive<Core<A>>>;

#[derive(Show, FromPrimitive, PartialEq, Eq)]
enum Lifecycle {
    // INITIAL - In the initial state. Has not been realized and neither the
    // consumer nor the producer have registered interest.
    New = 0,
    // The consumer has registered interest in the future.
    ConsumerWait = 1,
    // The producer has registered interest. The consumer has not yet.
    ProducerWait = 2,
    // A value has been provided, but the future core will be reused after the
    // value has been consumed.
    Ready = 3,
    // A value has been provided and the producer is waiting again.
    ReadyProducerWait = 4,
    // Call producer callback again once current callback returns
    ProducerNotify = 5,
    ProducerNotifyCanceled = 6,
    // Call consumer callback again once current callback returns
    ConsumerNotify = 7,
    // FINAL - The future has been canceled
    Canceled = 8,
}

#[cfg(test)]
mod test {
    use super::State;
    use std::mem;

    #[test]
    pub fn test_struct_sizes() {
        assert_eq!(mem::size_of::<State>(), mem::size_of::<uint>());
    }
}
