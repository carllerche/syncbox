#[allow(dead_code)]

use self::Lifecycle::*;
use self::WaitStrategy::*;
use super::{BoxedReceive, AsyncResult, AsyncError};
use std::{fmt, mem, ptr};
use std::num::FromPrimitive;
use std::sync::atomic;
use std::sync::atomic::{AtomicUint, Ordering};
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
pub struct Core<T: Send, E: Send, P: FromCore<T, E>> {
    ptr: *mut CoreInner<T, E, P>,
}

impl<T: Send, E: Send, P: FromCore<T, E>> Core<T, E, P> {
    pub fn new() -> Core<T, E, P> {
        let ptr = Box::new(CoreInner::<T, E, P>::new());
        Core { ptr: unsafe { mem::transmute(ptr) }}
    }

    pub fn with_value(val: AsyncResult<T, E>) -> Core<T, E, P> {
        let ptr = Box::new(CoreInner::<T, E, P>::with_value(val));
        Core { ptr: unsafe { mem::transmute(ptr) }}
    }

    pub fn consumer_await(&self) -> AsyncResult<T, E> {
        self.inner().consumer_await()
    }

    pub fn producer_await(&self) -> AsyncResult<P, ()> {
        self.inner().producer_await()
    }

    pub fn consumer_receive<F: FnOnce(AsyncResult<T, E>) + Send>(&self, f: F) {
        self.inner().consumer_receive(f);
    }

    pub fn producer_receive<F: FnOnce(AsyncResult<P, ()>) + Send>(&self, f: F) {
        self.inner().producer_receive(f);
    }

    pub fn complete(&self, val: AsyncResult<T, E>, last: bool) {
        self.inner().complete(val, last);
    }

    pub fn cancel(&self) {
        self.inner().cancel();
    }

    #[inline]
    fn inner(&self) -> &CoreInner<T, E, P> {
        unsafe { &*self.ptr }
    }

    #[inline]
    fn inner_mut(&mut self) -> &mut CoreInner<T, E, P> {
        unsafe { &mut *self.ptr }
    }

    fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
}

impl<T: Send, E: Send, P: FromCore<T, E>> Clone for Core<T, E, P> {
    fn clone(&self) -> Core<T, E, P> {
        // Increments ref count and returns a new core
        self.inner().core()
    }
}

#[unsafe_destructor]
impl<T: Send, E: Send, P: FromCore<T, E>> Drop for Core<T, E, P> {
    fn drop(&mut self) {
        // Handle memory already zeroed out
        if self.ptr.is_null() { return; }

        if self.inner().state.ref_dec(Release) != 1 {
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
                mem::size_of::<CoreInner<T, E, P>>(),
                mem::min_align_of::<CoreInner<T, E, P>>());
        }
    }
}

unsafe impl<T: Send, E: Send, P: FromCore<T, E>> Send for Core<T, E, P> {
}

/*
 *
 * ===== OptionCore =====
 *
 */

pub struct OptionCore<T: Send, E: Send, P: FromCore<T, E>> {
    core: Core<T, E, P>,
}

impl<T: Send, E: Send, P: FromCore<T, E>> OptionCore<T, E, P> {
    #[inline]
    pub fn new(core: Core<T, E, P>) -> OptionCore<T, E, P> {
        OptionCore { core: core }
    }

    #[inline]
    pub fn is_some(&self) -> bool {
        !self.core.is_null()
    }

    pub fn get(&self) -> &Core<T, E, P> {
        assert!(self.is_some(), "Core has already been consumed");
        &self.core
    }

    #[inline]
    pub fn take(&mut self) -> Core<T, E, P> {
        assert!(self.is_some(), "Core has already been consumed");
        unsafe { mem::replace(&mut self.core, mem::zeroed()) }
    }
}

impl<T: Send, E: Send, P: FromCore<T, E>> Clone for OptionCore<T, E, P> {
    fn clone(&self) -> OptionCore<T, E, P> {
        OptionCore { core: self.core.clone() }
    }
}

/*
 *
 * ===== FromCore =====
 *
 */

pub trait FromCore<T: Send, E: Send> : Send {
    fn from_core(core: Core<T, E, Self>) -> Self;
}

/*
 *
 * ===== CoreInner<T> =====
 *
 */


struct CoreInner<T: Send, E: Send, P: FromCore<T, E>> {
    state: AtomicState,
    consumer_wait: Option<WaitStrategy<T, E>>,
    producer_wait: Option<WaitStrategy<P, ()>>,
    val: AsyncResult<T, E>, // May be uninitialized
}

impl<T: Send, E: Send, P: FromCore<T, E>> CoreInner<T, E, P> {
    fn new() -> CoreInner<T, E, P> {
        CoreInner {
            state: AtomicState::new(),
            consumer_wait: None,
            producer_wait: None,
            val: unsafe { mem::uninitialized() },
        }
    }

    fn with_value(val: AsyncResult<T, E>) -> CoreInner<T, E, P> {
        CoreInner {
            state: AtomicState::of(Ready),
            consumer_wait: None,
            producer_wait: None,
            val: val,
        }
    }

    fn consumer_await(&self) -> AsyncResult<T, E> {
        let mut curr = self.state.load(Relaxed);

        // There already is a value, just return
        if !curr.is_ready() {
            if curr.is_consuming() {
                // If `await()` has been called from within a receive callback
                // for the same future, panic. This is not a supported scenario
                // and could cause deadlocks.
                panic!("cannot block thread when in a callback");
            }

            // Store the wait strategy
            self.put_consumer_wait(WaitStrategy::parked());

            // Exec wait strategy
            curr = self.consumer_wait(curr);

            while !curr.is_ready() {
                Thread::park();
                curr = self.state.load(Relaxed);
            }
        }

        // Get completed value
        self.consume_val(curr, false)
    }

    fn consumer_receive<F: FnOnce(AsyncResult<T, E>) + Send>(&self, f: F) {
        let mut curr = self.state.load(Relaxed);

        debug!("Core::receive; state={:?}", curr);

        // If the future is already complete, then there is no need to move the
        // callback to a Box. Consume the future result directly.
        if curr.is_ready() && !curr.is_consuming() {
            // Get the value transitioning the state to New / ProducerWait
            let val = self.consume_val(curr, true);

            debug!("  - Invoking consumer");
            f(val);

            curr = self.state.done_consuming(Relaxed);

            if curr.is_consumer_notify() {
                self.notify_consumer_loop(curr);
            }

            return;
        }

        // At this point, odds are that the callback will happen async. Move
        // the callback to a box.
        self.put_consumer_wait(Callback(Box::new(f)));

        // Execute wait strategy
        self.consumer_wait(curr);
    }

    // Transition the state to indicate that a consumer is waiting.
    //
    fn consumer_wait(&self, mut curr: State) -> State {
        let mut next;
        let mut notify_producer;

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
                    if curr.is_producing() {
                        curr.with_lifecycle(ProducerNotify)
                    } else {
                        notify_producer = true;
                        curr.with_lifecycle(ConsumerWait)
                    }
                }
                Ready | ReadyProducerWait => {
                    debug!("  - completing consumer");

                    // Handles the recursion case
                    return self.notify_consumer(curr);
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

        if notify_producer {
            // Use a fence to acquire the producer callback
            atomic::fence(Acquire);

            // Notify the producer
            next = self.notify_producer(next);
        }

        next
    }

    fn notify_consumer(&self, curr: State) -> State {
        // Already in a consumer callback, track that it should be invoked
        // again
        if curr.is_consuming() {
            return self.defer_consumer_notify(curr);
        }

        self.notify_consumer_loop(curr)
    }

    fn defer_consumer_notify(&self, mut curr: State) -> State {
        loop {
            let next = match curr.lifecycle() {
                Ready => curr.with_lifecycle(ConsumerNotify),
                _ => panic!("invalid state {:?}", curr.lifecycle()),
            };

            let actual = self.state.compare_and_swap(curr, next, Relaxed);

            if actual == curr {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                return next;
            }

            curr = actual;
        }
    }

    fn notify_consumer_loop(&self, mut curr: State) -> State {
        loop {
            match self.take_consumer_wait() {
                Callback(cb) => {
                    // Take the value, indicating to transition to consuming
                    let val = self.consume_val(curr, true);

                    // Invoke the callback
                    debug!("  - notifying consumer");
                    cb.receive_boxed(val);
                    debug!("  - consumer notified");

                    curr = self.state.done_consuming(Relaxed);

                    if curr.is_consumer_notify() {
                        continue;
                    }

                    return curr;
                }
                Parked(thread) => {
                    thread.unpark();
                    return curr;
                }
            }
        }
    }

    fn producer_await(&self) -> AsyncResult<P, ()> {
        let mut curr = self.state.load(Relaxed);

        debug!("Core::producer_await; state={:?}", curr);

        if !curr.is_producer_ready() {
            if curr.is_producing() {
                // If `await()` has been called from within a producer callback
                // for the same future, panic. This is not a supported scenario
                // and could cause deadlocks.
                panic!("cannot block thread when in a callback");
            }

            self.put_producer_wait(WaitStrategy::parked());

            curr = self.producer_wait(curr);

            while !curr.is_producer_ready() {
                Thread::park();
                curr = self.state.load(Relaxed);
            }
        }

        self.producer_result(curr)
    }

    fn producer_receive<F: FnOnce(AsyncResult<P, ()>) + Send>(&self, f: F) {
        let mut curr = self.state.load(Relaxed);

        debug!("Core::producer_receive; state={:?}", curr);

        // Don't recurse produce callbacks
        if !curr.is_producing() {
            if curr.is_consumer_wait() || curr.is_canceled() {
                // The producer callback is about to be invoked
                curr = self.state.produce(curr, true, Relaxed);

                debug!("  - Invoking producer");
                f(self.producer_result(curr));

                curr = self.state.done_producing(Relaxed);

                if curr.is_producer_notify() {
                    self.notify_producer_loop(curr);
                }

                return;
            }
        }

        // At this point, odds are that the callback will happen async. Move
        // the callback to a box.
        self.put_producer_wait(Callback(Box::new(f)));

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
                    if curr.is_producing() {
                        curr.with_lifecycle(ProducerNotifyCanceled)
                    } else {
                        notify_producer = true;
                        curr.with_lifecycle(Canceled)
                    }
                }
                Ready => {
                    read_val = true;
                    curr.with_lifecycle(Canceled)
                }
                ReadyProducerWait => {
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

    fn complete(&self, val: AsyncResult<T, E>, last: bool) {
        let mut curr = self.state.load(Relaxed);
        let mut next;

        debug!("Core::complete; state={:?}; last={:?}", curr, last);

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

        if curr.is_producing() {
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
            match self.take_producer_wait() {
                Callback(cb) => {
                    // Transition the state to track that a producer callback
                    // is being invoked
                    curr = self.state.produce(curr, true, Relaxed);

                    debug!("  - Invoking producer; state={:?}", curr);

                    // Invoke the callback
                    cb.receive_boxed(self.producer_result(curr));

                    // Track that the callback is done being invoked
                    curr = self.state.done_producing(Relaxed);

                    debug!("  - Producer invoked; state={:?}", curr);

                    if curr.is_producer_notify() {
                        continue;
                    }

                    return curr;
                }
                Parked(thread) => {
                    thread.unpark();
                    return curr;
                }
            }
        }
    }

    fn producer_result(&self, curr: State) -> AsyncResult<P, ()> {
        if !curr.is_canceled() {
            Ok(FromCore::from_core(self.core()))
        } else {
            Err(AsyncError::canceled())
        }
    }

    fn consume_val(&self, curr: State, callback: bool) -> AsyncResult<T, E> {
        // Ensure that the memory is synced
        atomic::fence(Acquire);

        // Get the value
        let ret = unsafe { self.take_val() };

        // Transition the state
        self.state.consume(curr, callback, Relaxed);

        ret
    }

    unsafe fn put_val(&self, val: AsyncResult<T, E>) {
        ptr::write(mem::transmute(&self.val), val);
    }

    unsafe fn take_val(&self) -> AsyncResult<T, E> {
        ptr::read(&self.val)
    }

    fn put_consumer_wait(&self, wait: WaitStrategy<T, E>) {
        unsafe {
            let s: &mut CoreInner<T, E, P> = mem::transmute(self);
            s.consumer_wait = Some(wait);
        }
    }

    fn take_consumer_wait(&self) -> WaitStrategy<T, E> {
        unsafe {
            let s: &mut CoreInner<T, E, P> = mem::transmute(self);
            s.consumer_wait.take().expect("consumer_wait is none")
        }
    }

    fn put_producer_wait(&self, wait: WaitStrategy<P, ()>) {
        unsafe {
            let s: &mut CoreInner<T, E, P> = mem::transmute(self);
            s.producer_wait = Some(wait);
        }
    }

    fn take_producer_wait(&self) -> WaitStrategy<P, ()> {
        unsafe {
            let s: &mut CoreInner<T, E, P> = mem::transmute(self);
            s.producer_wait.take().expect("producer_wait is none")
        }
    }

    fn core(&self) -> Core<T, E, P> {
        // Using a relaxed ordering is alright here, as knowledge of the original reference
        // prevents other threads from erroneously deleting the object.
        //
        // As explained in the [Boost documentation][1], Increasing the reference counter can
        // always be done with memory_order_relaxed: New references to an object can only be formed
        // from an existing reference, and passing an existing reference from one thread to another
        // must already provide any required synchronization.
        //
        // [1]: (www.boost.org/doc/libs/1_55_0/doc/html/atomic/usage_examples.html)
        self.state.ref_inc(Relaxed);
        Core { ptr: unsafe { mem::transmute(self) } }
    }
}

struct AtomicState {
    atomic: AtomicUint,
}

impl AtomicState {
    fn new() -> AtomicState {
        let initial = State::new();
        AtomicState { atomic: AtomicUint::new(initial.as_uint()) }
    }

    fn of(lifecycle: Lifecycle) -> AtomicState {
        let initial = State::new().with_lifecycle(lifecycle);
        AtomicState { atomic: AtomicUint::new(initial.as_uint()) }
    }

    fn load(&self, order: Ordering) -> State {
        let val = self.atomic.load(order);
        State::load(val)
    }

    fn compare_and_swap(&self, old: State, new: State, order: Ordering) -> State {
        let ret = self.atomic.compare_and_swap(old.as_uint(), new.as_uint(), order);
        State::load(ret)
    }

    fn ref_inc(&self, order: Ordering) {
        self.atomic.fetch_add(1 << REF_COUNT_OFFSET, order);
    }

    fn ref_dec(&self, order: Ordering) -> uint {
        let prev = self.atomic.fetch_sub(1 << REF_COUNT_OFFSET, order);
        prev >> REF_COUNT_OFFSET
    }

    fn consume(&self, mut curr: State, callback: bool, order: Ordering) -> State {
        loop {
            let next = match curr.lifecycle() {
                Ready | ConsumerNotify => {
                    if callback {
                        curr.with_lifecycle(New).with_consuming()
                    } else {
                        curr.with_lifecycle(New)
                    }
                }
                ReadyProducerWait => {
                    if callback {
                        curr.with_lifecycle(ProducerWait).with_consuming()
                    } else {
                        curr.with_lifecycle(ProducerWait)
                    }
                }
                _ => panic!("unexpected state {:?}", curr),
            };

            let actual = self.compare_and_swap(curr, next, order);

            if curr == actual {
                debug!("  - transitioned from {:?} to {:?} (consuming value)", curr, next);
                return next;
            }

            curr = actual
        }
    }

    fn done_consuming(&self, order: Ordering) -> State {
        let val = self.atomic.fetch_sub(CONSUMING_MASK, order);
        State { val: val - CONSUMING_MASK }
    }

    fn produce(&self, mut curr: State, callback: bool, order: Ordering) -> State {
        loop {
            let next = match curr.lifecycle() {
                ConsumerWait | ProducerNotify => {
                    if callback {
                        curr.with_lifecycle(ConsumerWait).with_producing()
                    } else {
                        curr.with_lifecycle(ConsumerWait)
                    }
                }
                Canceled => {
                    if callback {
                        curr.with_producing()
                    } else {
                        curr
                    }
                }
                _ => panic!("unexpected state {:?}", curr),
            };

            let actual = self.compare_and_swap(curr, next, order);

            if curr == actual {
                debug!("  - transitioned from {:?} to {:?}", curr, next);
                return next;
            }

            curr = actual
        }
    }

    fn done_producing(&self, order: Ordering) -> State {
        let val = self.atomic.fetch_sub(PRODUCING_MASK, order);
        State { val: val - PRODUCING_MASK }
    }
}

#[derive(Copy, Eq, PartialEq)]
struct State {
    val: uint,
}

const LIFECYCLE_MASK:   uint = 15;
const CONSUMING_MASK:   uint = 1 << 4;
const PRODUCING_MASK:   uint = 1 << 5;
const REF_COUNT_OFFSET: uint = 6;

impl State {
    fn new() -> State {
        State { val: 1 << REF_COUNT_OFFSET }
    }

    fn load(val: uint) -> State {
        State { val: val }
    }

    fn ref_count(&self) -> uint {
        self.val >> REF_COUNT_OFFSET
    }

    fn lifecycle(&self) -> Lifecycle {
        FromPrimitive::from_uint(self.val & LIFECYCLE_MASK)
            .expect("unexpected state value")
    }

    fn with_lifecycle(&self, val: Lifecycle) -> State {
        let val = self.val & !LIFECYCLE_MASK | val as uint;
        State { val: val }
    }

    fn with_consuming(&self) -> State {
        State { val: self.val | CONSUMING_MASK }
    }

    fn with_producing(&self) -> State {
        State { val: self.val | PRODUCING_MASK }
    }

    fn is_consuming(&self) -> bool {
        self.val & CONSUMING_MASK == CONSUMING_MASK
    }

    fn is_producing(&self) -> bool {
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

    fn as_uint(self) -> uint {
        self.val
    }
}

impl fmt::Show for State {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "State[refs={}; consuming={}; producing={}; lifecycle={:?}]",
               self.ref_count(), self.is_consuming(), self.is_producing(), self.lifecycle())
    }
}

enum WaitStrategy<T, E> {
    Callback(Box<BoxedReceive<T, E>>),
    Parked(Thread),
}

impl<T, E> WaitStrategy<T, E> {
    fn parked() -> WaitStrategy<T, E> {
        Parked(Thread::current())
    }
}

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
    use super::{Core, State};
    use std::mem;
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    pub fn test_struct_sizes() {
        assert_eq!(mem::size_of::<State>(), mem::size_of::<uint>());
    }

    #[test]
    pub fn test_initial_ref_count() {
        use util::async::Complete;

        let c = Core::<uint, (), Complete<uint, ()>>::new();
        assert_eq!(1, c.inner().state.load(Relaxed).ref_count());
    }
}
