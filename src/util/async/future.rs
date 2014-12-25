//! A basic implementation of futures.
//!
//! As of now, the implementation is fairly naive. A single mutex is used to
//! handle synchronization and the implementation generally doesn't worry that
//! much about performance at the moment. The goal currently is to figure out
//! the API and then worry about performance. Eventually Future will be
//! re-implementing using lock-free strategies and focus on performance.

// TODO:
// - Interest::cancel()
// - Deref

use super::{
    Cancel,
    Async,
    AsyncResult,
    AsyncError,
};
use super::AsyncErrorKind::*;
use self::State::*;
use self::WaitStrategy::*;

use std::{fmt, mem};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::Thread;

pub struct Future<T> {
    inner: Option<FutureInner<T>>,
}

impl<T: Send> Future<T> {
    pub fn pair() -> (Future<T>, Producer<T>) {
        let inner = FutureInner::new();

        let f = Future::new(inner.clone());
        let c = Producer::new(inner);

        (f, c)
    }

    pub fn of(val: T) -> Future<T> {
        Future::new(FutureInner::of(val))
    }

    /// Creates a new Future with the given core
    #[inline]
    fn new(inner: FutureInner<T>) -> Future<T> {
        Future { inner: Some(inner) }
    }

    #[inline]
    pub fn await(mut self) -> AsyncResult<T> {
        self.inner.take().unwrap().await()
    }

    pub fn is_ready(&self) -> bool {
        self.inner.as_ref().unwrap()
            .is_ready()
    }

    pub fn is_err(&self) -> bool {
        self.inner.as_ref().unwrap()
            .is_err()
    }

    #[inline]
    pub fn unwrap(self) -> T {
        self.await().unwrap()
    }

    /*
     *
     * ===== Operations =====
     *
     */

    /// Maps a `Future<T>` to `Future<U>` by applying a function to a realized
    /// value, leaving error values untouched.
    pub fn map<U: Send, F: FnOnce(T) -> U + Send>(self, cb: F) -> Future<U> {
        let (ret, prod) = Future::pair();

        prod.ready(move |prod| {
            self.ready(move |f| {
                match f.await() {
                    Ok(v) => prod.complete(cb(v)),
                    Err(_) => prod.fail("origin failed"), // TODO: better errors
                }
            });
        });


        ret
    }
}

impl<T: Send> Async<Interest> for Future<T> {
    #[inline]
    fn ready<F: FnOnce(Future<T>) + Send>(mut self, cb: F) -> Interest {
        self.inner.take().unwrap().ready(cb);
        Interest
    }
}

impl<T: Send + Clone> Clone for Future<T> {
    fn clone(&self) -> Future<T> {
        // Used to clone the value
        fn cloner<T: Clone>(val: &AsyncResult<T>) -> AsyncResult<T> {
            match *val {
                Ok(ref v) => Ok(v.clone()),
                Err(e) => Err(e),
            }
        }

        if let Some(ref inner) = self.inner {
            inner.consumer_clone(cloner);
            return Future::new(inner.clone())
        }

        panic!("[BUG] attempting to clone a consumed future value");
    }
}

#[unsafe_destructor]
impl<T: Send> Drop for Future<T> {
    fn drop(&mut self) {
        match self.inner.take() {
            Some(inner) => inner.cancel(),
            None => {}
        }
    }
}

impl<T: fmt::Show> fmt::Show for Future<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(fmt, "Future"));
        Ok(())
    }
}

#[deriving(Copy)]
pub struct Interest;

impl Cancel for Interest {
    fn cancel(self) {
        unimplemented!()
    }
}

pub struct Producer<T> {
    inner: Option<FutureInner<T>>,
}

impl<T: Send> Producer<T> {
    /// Creates a new Producer with the given core
    #[inline]
    fn new(inner: FutureInner<T>) -> Producer<T> {
        Producer { inner: Some(inner) }
    }

    #[inline]
    pub fn complete(mut self, val: T) {
        self.inner.take().unwrap().complete(Ok(val));
    }

    #[inline]
    pub fn fail(mut self, desc: &'static str) {
        let inner = self.inner.take().unwrap();
        inner.complete(Err(AsyncError {
            kind: ExecutionError,
            desc: desc,
        }));
    }

    pub fn await(mut self) -> AsyncResult<Producer<T>> {
        let inner = self.inner.take().unwrap();
        inner.producer_await()
    }

    pub fn is_ready(&self) -> bool {
        self.inner.as_ref()
            .expect("producer has already been consumed")
            .producer_is_ready()
    }

    pub fn is_err(&self) -> bool {
        self.inner.as_ref()
            .expect("producer has already been consumed")
            .producer_is_err()
    }
}

#[unsafe_destructor]
impl<T: Send> Drop for Producer<T> {
    fn drop(&mut self) {
        match self.inner.take() {
            Some(inner) => {
                inner.complete(Err(AsyncError {
                    kind: ExecutionError,
                    desc: "producer dropped out of scope without completing",
                }));
            }
            None => {}
        }
    }
}

/*
 *
 * ===== Consumer interest future =====
 *
 */

impl<T: Send> Async<ProducerInterest> for Producer<T> {
    #[inline]
    fn ready<F: FnOnce(Producer<T>) + Send>(mut self, cb: F) -> ProducerInterest {
        let inner = self.inner.take().unwrap();
        inner.producer_ready(cb);
        ProducerInterest
    }
}

impl<T: Send> Cancel for Producer<T> {
    #[inline]
    fn cancel(self) {
        self.fail("canceled by producer");
    }
}

#[deriving(Copy)]
pub struct ProducerInterest;

impl Cancel for ProducerInterest {
    fn cancel(self) {
        unimplemented!()
    }
}

/*
 *
 * ===== Implementation details =====
 *
 */

// == Implementation details ==
//
// FutureInner handles the implementation of Future & Producer. It manages
// completing the value for the primary Future instance as well as any
// clones.
//
// FutureInner consists of a state machine that manages the internal state of
// the future. State transitions are a DAG, aka there are no possible cycles.
// The state transitions are as follows:
//
// * Pending
//      -> ConsumerWait: The consumer expressed interest in the value.
//      -> ProducerWait: The producer is waiting for consumer interest.
//      -> Complete:     The producer has completed the future
//      -> Canceled:     The consumer has canceled the future
//
// * ConsumerWait
//      -> Complete: The future has been completed, notify waiting consumers
//      -> Canceled: The consumer has canceled interest in the future
//
// * ProducerWait
//      -> ConsumerWait: The consumer has expressed interest in the value
//      -> Canceled:     The consumer has canceled the future
//
// Other states are final.
//
// # Notes on transitions:
//
// - It's possible to go from ConsumerWait -> Canceled if the consumer does
//   take_timed then cancels or does a receive() followed by a cancelation.
//
// # Cloning
//
// If type T implements Clone then Future can be cloned. FutureInner will
// track the number of interested consumers. Then, once the future has been
// realized and consumers start taking the result, FutureInner will clone the
// value then. For the last consumer, the value will be moved out.
//
// The future will transition to complete only once all the interested
// consumers have received the value.
//
// The future will transition to canceled only once all the interested
// consumers have issued a cancel.

struct FutureInner<T> {
    core: Arc<Mutex<Core<T>>>,
}

impl<T: Send> FutureInner<T> {
    fn new() -> FutureInner<T> {
        FutureInner {
            core: Arc::new(Mutex::new(Core::new()))
        }
    }

    fn of(val: T) -> FutureInner<T> {
        FutureInner {
            core: Arc::new(Mutex::new(Core::completed(val))),
        }
    }

    fn is_ready(&self) -> bool {
        let core = self.lock();

        // This fn should not be called if interest has been canceled
        assert!(!core.state.is_canceled());
        core.state.is_complete()
    }

    fn is_err(&self) -> bool {
        let core = self.lock();
        core.is_err()
    }

    fn ready<F: FnOnce(Future<T>) + Send>(self, cb: F) {
        // Acquire the lock
        let mut core = self.lock();

        // If the producer is currently waiting, notify it that the
        // consumer has indicated interest in the result.
        core = self.producer_notify(core, true);

        // If the future has already been realized, move the value out
        // of the core so that it can be sent to the supplied callback.
        if core.state.is_complete() {
            // Drop the lock before invoking the callback (prevent
            // deadlocks).
            drop(core);
            // TODO: Avoid the clone
            cb(Future::new(self.clone()));
            return;
        }

        // The future's value has not yet been realized. Save off the
        // callback and mark the consumer as waiting for the value. When
        // the value is available, the calback will be invoked with it.
        let i: Box<Invoke<(Future<T>,), ()> + Send> = box cb;
        core.push_consumer_wait(Callback(i));
    }

    fn await(&self) -> AsyncResult<T> {
        // Acquire the lock
        let mut core = self.lock();

        // If the producer is currently waiting, notify it that the
        // consumer has indicated interest in the result.
        core = self.producer_notify(core, true);

        // If the future is complete, return immediately.
        if let Some(val) = core.take_value() {
            return val;
        }

        // Before the thread blocks, track that the consumer is waiting
        core.push_consumer_wait(Park(Thread::current()));

        // Checking the value and waiting happens in a loop to handle
        // cases where the condition variable unblocks early for an
        // unknown reason (permitted by the pthread spec).
        loop {
            // Check if the value has been realized before blocking
            if let Some(val) = core.take_value() {
                return val;
            }

            drop(core);
            Thread::park();
            core = self.lock();
        }
    }

    fn cancel(&self) {
        // Acquire the lock
        let mut core = self.lock();

        if core.state.is_pending() || core.state.is_producer_wait() {
            // If the producer is currently waiting, notify it that the
            // consumer has canceled the future.
            core = self.producer_notify(core, false);

            // The state MAY have already transitioned to Canceled, but not
            // always.
            core.state = Canceled;
        }
    }

    fn complete(self, val: AsyncResult<T>) {
        // Acquire the lock
        let mut core = self.lock();

        // Store off the value
        core.put(val);

        // Check if the consumer is waiting on the value, if so, it will
        // be notified that value is ready.
        if let ConsumerWait(waiters) = core.take_consumer_wait() {
            for waiter in waiters.into_iter() {
                // Check the consumer wait strategy
                match waiter {
                    // If the consumer is waiting with a callback, release
                    // the lock and invoke the callback with the value.
                    Some(Callback(cb)) => {
                        // Relase the lock
                        drop(core);
                        // Invoke the callback
                        cb.invoke((Future::new(self.clone()),));
                        // Reacquire the lock in case there is another iteration
                        core = self.lock();
                    }
                    // Otherwise, store the value on the future and signal
                    // the consumer that the value is ready.
                    Some(Park(thread)) => {
                        // Signal that the future is complete
                        thread.unpark();
                    }
                    None => {}
                }
            }

            return;
        }

        core.state = Complete;
    }

    fn consumer_clone(&self, cloner: Cloner<T>) {
        let mut core = self.lock();

        core.clones += 1;

        if core.cloner.is_none() {
            core.cloner = Some(cloner);
        }
    }

    fn producer_is_ready(&self) -> bool {
        let core = self.lock();
        core.state.is_consumer_ready() || core.state.is_consumer_wait()
    }

    fn producer_is_err(&self) -> bool {
        let core = self.lock();
        core.state.is_canceled()
    }

    fn producer_ready<F: FnOnce(Producer<T>) + Send>(self, cb: F) {
        // Run the synchronized logic within a scope such that the lock
        // is released at the end of the scope.
        {
            // Acquire the lock
            let mut core = self.lock();

            // If the consumer has not registered an interest yet, save off
            // the callback for when it does and return;
            if core.state.is_pending() || core.state.is_consumer_ready() {
                core.state = ProducerWait(Callback(box cb));
                return;
            }

            // The consumer has registered an interest in the value. Release
            // the lock then invoke the callback. This allows the callback
            // to run outside of the lock preventing deadlocks.
            drop(core);
        }

        cb(Producer::new(self));
    }

    fn producer_await(self) -> AsyncResult<Producer<T>> {
        let mut canceled;

        // Run the synchronized logic within a scope such that the lock
        // is released at the end of the scope.
        {
            // Acquire the lock
            let mut core = self.lock();

            // If the consumer has not registered an interest yet, track
            // that the producer is about to block, then wait for the
            // signal.
            if core.state.is_pending() {
                core.state = ProducerWait(Park(Thread::current()));

                // Loop as long as the future remains in the producer wait
                // state.
                loop {
                    drop(core);
                    Thread::park();
                    core = self.lock();

                    // If the future state has changed, break out fo the
                    // loop.
                    if !core.state.is_producer_wait() {
                        canceled = core.state.is_canceled();
                        break;
                    }
                }
            } else {
                canceled = core.state.is_canceled();
            }
        }

        if canceled {
            // Return the fact that the consumer has canceled interest
            // for the future
            Err(cancelation())
        } else {
            // Return the producer (simply wrap the FutureInner instance)
            Ok(Producer::new(self))
        }
    }

    fn producer_notify<'a>(&'a self, mut core: LockedCore<'a, T>, interest: bool)
            -> LockedCore<'a, T> {

        // Run notification in a loop, the callback has the option to
        // re-register another receive callback, in which case it should
        // be immediately invoked.
        loop {
            if let ProducerWait(strategy) = core.take_producer_wait(interest) {
                match strategy {
                    Callback(cb) => {
                        drop(core);
                        cb.invoke((Producer::new(self.clone()),));
                        core = self.lock();
                    }
                    Park(thread) => thread.unpark(),
                }
            } else {
                break;
            }
        }

        core
    }

    #[inline]
    fn lock<'a>(&'a self) -> MutexGuard<'a, Core<T>> {
        self.core.lock()
    }
}

impl<T: Send> Clone for FutureInner<T> {
    fn clone(&self) -> FutureInner<T> {
        FutureInner { core: self.core.clone() }
    }
}

struct Core<T> {
    // The realized value
    val: Option<AsyncResult<T>>,
    // State of the future
    state: State<T>,
    // Number of clones
    clones: uint,
    // Clone function
    cloner: Option<Cloner<T>>,
}

type LockedCore<'a, T> = MutexGuard<'a, Core<T>>;
type Cloner<T> = fn(&AsyncResult<T>) -> AsyncResult<T>;

impl<T: Send> Core<T> {
    fn new() -> Core<T> {
        Core {
            val: None,
            state: Pending,
            // There always is one consumer at creation
            clones: 1,
            // Gets set when clone is invoked
            cloner: None,
        }
    }

    fn completed(val: T) -> Core<T> {
        Core {
            val: Some(Ok(val)),
            state: Complete,
            clones: 1,
            cloner: None,
        }
    }

    fn is_err(&self) -> bool {
        self.val.as_ref()
            .map(|v| v.is_err())
            .unwrap_or(false)
    }

    fn put(&mut self, val: AsyncResult<T>) {
        assert!(self.val.is_none(), "future already completed");
        self.val = Some(val);
    }

    fn take_value(&mut self) -> Option<AsyncResult<T>> {
        if self.val.is_none() {
            return None;
        }

        assert!(self.clones > 0, "future has been consumed");
        self.clones -= 1;

        if self.clones == 0 {
            mem::replace(&mut self.val, None)
        } else {
            if let Some(cloner) = self.cloner {
                // Clone the value
                Some(cloner(self.val.as_ref().unwrap()))
            } else {
                panic!("[BUG] multiple clones tracked but no cloner function present");
            }
        }
    }

    fn take_consumer_wait(&mut self) -> State<T> {
        if self.state.is_consumer_wait() {
            mem::replace(&mut self.state, Complete)
        } else {
            Complete
        }
    }

    fn push_consumer_wait(&mut self, strategy: WaitStrategy<Future<T>>) -> uint {
        if let ConsumerWait(ref mut vec) = self.state {
            let ret = vec.len();
            vec.push(Some(strategy));
            ret
        } else {
            self.state = ConsumerWait(vec![Some(strategy)]);
            0
        }
    }

    fn take_producer_wait(&mut self, interest: bool) -> State<T> {
        if self.state.is_producer_wait() {
            let new = if interest {
                ConsumerReady
            } else {
                Canceled
            };

            mem::replace(&mut self.state, new)
        } else {
            Pending
        }
    }
}

enum State<T> {
    Pending,
    ConsumerReady, // Consumer has signaled interest but hasn't blocked
    ConsumerWait(Vec<Option<WaitStrategy<Future<T>>>>),
    ProducerWait(WaitStrategy<Producer<T>>),
    Complete,
    Canceled,
}

impl<T: Send> State<T> {
    fn is_pending(&self) -> bool {
        match *self {
            Pending => true,
            _ => false,
        }
    }

    fn is_canceled(&self) -> bool {
        match *self {
            Canceled => true,
            _ => false,
        }
    }

    fn is_consumer_ready(&self) -> bool {
        match *self {
            ConsumerReady => true,
            _ => false,
        }
    }

    fn is_consumer_wait(&self) -> bool {
        match *self {
            ConsumerWait(..) => true,
            _ => false,
        }
    }

    fn is_producer_wait(&self) -> bool {
        match *self {
            ProducerWait(..) => true,
            _ => false,
        }
    }

    fn is_complete(&self) -> bool {
        match *self {
            Complete => true,
            _ => false,
        }
    }
}

impl<T> fmt::Show for State<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", match *self {
            Pending => "Pending",
            Complete => "Complete",
            Canceled => "Canceled",
            ConsumerReady => "ConsumerReady",
            ConsumerWait(..) => "ConsumerWait",
            ProducerWait(ref strategy) => {
                match *strategy {
                    Park(..) => "ProducerWait(Park)",
                    Callback(..) => "ProducerWait(Callback)",
                }
            }
        })
    }
}

enum WaitStrategy<T> {
    Callback(Box<Invoke<(T,), ()> + Send>),
    Park(Thread),
}

trait Invoke<A, R> {
    fn invoke(self: Box<Self>, arg: A) -> R;
}

impl<A, R, F> Invoke<A,R> for F where F : FnOnce<A, R> {
    fn invoke(self: Box<F>, arg: A) -> R {
        let f = *self;
        f.call_once(arg)
    }
}

fn cancelation() -> AsyncError {
    AsyncError {
        kind: CancelationError,
        desc: "future was canceled by consumer",
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use util::async::Async;
    use std::io::timer::sleep;
    use std::time::Duration;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUint, Relaxed};
    use std::thread::Thread;

    /*
     *
     * ===== Core functionality tests =====
     *
     */

    #[test]
    pub fn test_complete_before_await() {
        let (f, c) = Future::pair();

        spawn(move || c.complete("zomg"));

        sleep(millis(50));
        assert_eq!(f.unwrap(), "zomg");
    }

    #[test]
    pub fn test_complete_after_await() {
        let (f, c) = Future::pair();

        spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        });

        assert_eq!(f.unwrap(), "zomg");
    }

    #[test]
    pub fn test_complete_before_receive() {
        let (f, c) = Future::pair();
        let (tx, rx) = channel();

        spawn(move || c.complete("zomg"));

        sleep(millis(50));
        f.ready(move |f| tx.send(f.unwrap()));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_complete_after_receive() {
        let (f, c) = Future::pair();
        let (tx, rx) = channel();

        spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        });

        f.ready(move |f| tx.send(f.unwrap()));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_future_before_await() {
        let (f, c) = Future::pair();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        c.ready(move |c| {
            assert!(c.is_ready());
            assert!(!c.is_err());
            assert!(w2.load(Relaxed));

            c.complete("zomg");
        });

        w1.store(true, Relaxed);
        assert_eq!(f.unwrap(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_future_after_await() {
        let (f, c) = Future::pair();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        spawn(move || {
            sleep(millis(50));

            c.ready(move |c| {
                assert!(w2.load(Relaxed));
                assert!(c.is_ready());
                c.complete("zomg");
            });
        });

        w1.store(true, Relaxed);
        assert_eq!(f.unwrap(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_before_consumer_receive() {
        let (f, c) = Future::pair();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        c.ready(move |c| {
            assert!(w2.load(Relaxed));
            assert!(c.is_ready());
            c.complete("zomg");
        });

        let (tx, rx) = channel();
        w1.store(true, Relaxed);

        f.ready(move |msg| {
            assert_eq!("zomg", msg.unwrap());
            tx.send("hi2u");
        });

        assert_eq!("hi2u", rx.recv());
    }

    #[test]
    pub fn test_receive_complete_after_consumer_receive() {
        let (f, c) = Future::pair();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        spawn(move || {
            sleep(millis(50));

            c.ready(move |c| {
                assert!(w2.load(Relaxed));
                assert!(c.is_ready());
                c.complete("zomg");
            });
        });

        let (tx, rx) = channel();
        w1.store(true, Relaxed);

        f.ready(move |msg| {
            assert_eq!("zomg", msg.unwrap());
            tx.send("hi2u");
        });

        assert_eq!("hi2u", rx.recv());
    }

    #[test]
    pub fn test_await_complete_before_consumer_await() {
        let (f, c) = Future::pair();

        spawn(move || c.await().unwrap().complete("zomg"));
        sleep(millis(50));

        assert_eq!("zomg", f.unwrap());
    }

    #[test]
    pub fn test_await_complete_after_consumer_await() {
        let (f, c) = Future::pair();

        spawn(move || {
            sleep(millis(50));
            c.await().unwrap().complete("zomg");
        });

        assert_eq!("zomg", f.unwrap());
    }

    #[test]
    pub fn test_await_complete_before_consumer_receive() {
        let (f, c) = Future::pair();
        let (tx, rx) = channel();

        spawn(move || c.await().unwrap().complete("zomg"));
        sleep(millis(50));

        f.ready(move |f| tx.send(f.unwrap()));

        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_await_complete_after_consumer_receive() {
        let (f, c) = Future::pair();
        let (tx, rx) = channel();

        spawn(move || {
            sleep(millis(50));
            c.await().unwrap().complete("zomg");
        });

        f.ready(move |f| tx.send(f.unwrap()));
        assert_eq!(rx.recv(), "zomg");
    }

    // Utility method used below
    fn waiting(count: uint, d: Arc<AtomicUint>, c: Producer<&'static str>) {
        // Assert that the callback is not invoked recursively
        assert_eq!(0, d.fetch_add(1, Relaxed));

        if count == 5 {
            c.complete("done");
        } else {
            let d2 = d.clone();
            c.ready(move |c| waiting(count + 1, d2, c));
        }

        d.fetch_sub(1, Relaxed);
    }

    #[test]
    pub fn test_producer_receive_when_consumer_cb_set() {
        let (f, c) = Future::pair();
        let (tx, rx) = channel();
        let depth = Arc::new(AtomicUint::new(0));

        waiting(0, depth, c);

        f.ready(move |f| tx.send(f.unwrap()));
        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_producer_receive_when_consumer_waiting() {
        let (f, c) = Future::pair();
        let depth = Arc::new(AtomicUint::new(0));

        waiting(0, depth, c);

        sleep(millis(50));
        assert_eq!(f.unwrap(), "done");
    }

    #[test]
    pub fn test_producer_take_when_consumer_cb_set() {
        let (f, c) = Future::pair();
        let (tx, rx) = channel();

        spawn(move || {
            c.await().unwrap()
                .await().unwrap()
                .await().unwrap().complete("zomg");
        });

        sleep(millis(50));

        f.ready(move |f| tx.send(f.unwrap()));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_producer_take_when_consumer_waiting() {
        let (f, c) = Future::pair();

        spawn(move || {
            c.await().unwrap()
                .await().unwrap()
                .await().unwrap().complete("zomg");
        });

        sleep(millis(50));
        assert_eq!(f.unwrap(), "zomg");
    }

    #[test]
    pub fn test_canceling_future_before_producer_receive() {
        let (f, c) = Future::<uint>::pair();
        let (tx, rx) = channel();

        drop(f);

        c.ready(move |c| {
            assert!(c.is_err());
            tx.send("done");
        });

        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_canceling_future_before_producer_take() {
        let (f, c) = Future::<uint>::pair();

        drop(f);

        assert!(c.await().is_err());
    }

    #[test]
    pub fn test_canceling_future_after_producer_receive() {
        let (f, c) = Future::<uint>::pair();
        let (tx, rx) = channel();

        c.ready(move |c| {
            assert!(c.is_err());
            tx.send("done");
        });

        drop(f);
        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_canceling_future_after_producer_take() {
        let (f, c) = Future::<uint>::pair();
        let (tx, rx) = channel();

        spawn(move || {
            assert!(c.await().is_err());
            tx.send("done");
        });

        sleep(millis(50));
        drop(f);

        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_canceling_producer_then_ready() {
        let (f, c) = Future::<uint>::pair();
        let (tx, rx) = channel();

        drop(c);

        f.ready(move |f| {
            // TODO: More assertions
            assert!(f.is_err());
            tx.send("done");
        });

        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_producer_fail_before_consumer_take() {
        let (f, c) = Future::<uint>::pair();

        c.fail("nope");

        let err = f.await().unwrap_err();
        assert_eq!(err.desc, "nope");
        assert!(err.is_execution_error());
    }

    #[test]
    pub fn test_producer_fail_consumer_receive() {
        let (f, c) = Future::<uint>::pair();

        spawn(move || {
            sleep(millis(50));
            c.fail("nope");
        });

        let err = f.await().unwrap_err();
        assert_eq!(err.desc, "nope");
        assert!(err.is_execution_error());
    }

    #[test]
    pub fn test_producer_drops_before_consumer_take() {
        let (f, c) = Future::<uint>::pair();

        drop(c);

        let err = f.await().unwrap_err();
        assert_eq!(err.desc, "producer dropped out of scope without completing");
        assert!(err.is_execution_error());
    }

    #[test]
    pub fn test_producer_drops_after_consumer_take() {
        let (f, c) = Future::<uint>::pair();

        spawn(move || {
            sleep(millis(50));
            drop(c);
        });

        let err = f.await().unwrap_err();
        assert_eq!(err.desc, "producer dropped out of scope without completing");
        assert!(err.is_execution_error());
    }

    // ======= Cloning futures =======

    #[test]
    pub fn test_cloning_clone_complete_take_both() {
        let (f1, c) = Future::pair();
        let f2 = f1.clone();

        c.complete("zomg");

        assert_eq!(f1.unwrap(), "zomg");
        assert_eq!(f2.unwrap(), "zomg");
    }

    #[test]
    pub fn test_cloning_clone_take_both_complete() {
        let (f1, c) = Future::pair();
        let f2 = f1.clone();

        let (tx, rx) = channel();

        spawn(move || tx.send(f2.unwrap()));
        spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        });

        assert_eq!(f1.unwrap(), "zomg");
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_cloning_clone_take_complete_take() {
        let (f1, c) = Future::pair();
        let f2 = f1.clone();

        spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        });

        assert_eq!(f1.unwrap(), "zomg");
        assert_eq!(f2.unwrap(), "zomg");
    }

    #[test]
    pub fn test_cloning_clone_complete_receive_both() {
        let (f1, c) = Future::pair();
        let f2 = f1.clone();

        c.complete("zomg");

        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        f1.ready(move |f| tx1.send(f.unwrap()));
        f2.ready(move |f| tx2.send(f.unwrap()));

        assert_eq!(rx1.recv(), "zomg");
        assert_eq!(rx2.recv(), "zomg");
    }

    #[test]
    pub fn test_cloning_receive_both_complete() {
        let (f1, c) = Future::pair();
        let f2 = f1.clone();

        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        f1.ready(move |f| tx1.send(f.unwrap()));
        f2.ready(move |f| tx2.send(f.unwrap()));

        c.complete("zomg");

        assert_eq!(rx1.recv(), "zomg");
        assert_eq!(rx2.recv(), "zomg");
    }

    #[test]
    pub fn test_cloning_receive_complete_receive() {
        let (f1, c) = Future::pair();
        let f2 = f1.clone();

        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        f1.ready(move |f| tx1.send(f.unwrap()));
        c.complete("zomg");
        f2.ready(move |f| tx2.send(f.unwrap()));


        assert_eq!(rx1.recv(), "zomg");
        assert_eq!(rx2.recv(), "zomg");
    }

    /*
     *
     * ===== Future::of() =====
     *
     */

    #[test]
    pub fn test_future_of_is_ready() {
        let f = Future::of("hello");

        assert!(f.is_ready());
        assert!(!f.is_err());

        let (tx, rx) = channel();

        f.ready(move |f| {
            assert!(f.is_ready());
            assert!(!f.is_err());
            tx.send("done")
        });

        assert_eq!("done", rx.recv());
    }

    #[test]
    pub fn test_future_of_await() {
        let f = Future::of("hello");
        assert_eq!("hello", f.unwrap());
    }

    /*
     *
     * ===== Future::map() =====
     *
     */

    #[test]
    pub fn test_ready_future_map() {
        let f = Future::of(1u)
            .map(move |v| v+v);

        assert_eq!(2u, f.unwrap());
    }

    #[test]
    #[ignore] // TODO: Currently broken
    pub fn test_future_map_then_ready() {
        let (f, p) = Future::<uint>::pair();

        let f = f.map(move |v| v+v);
        p.complete(1u);

        assert!(f.is_ready());
        assert_eq!(2u, f.unwrap());
    }

    #[test]
    pub fn test_future_interest_registered_on_map() {
        let (f, p) = Future::<uint>::pair();
        let (tx, rx) = channel();

        let f = f.map(move |v| v+v);

        assert!(!p.is_ready());

        f.ready(move |f| tx.send(f.unwrap()));
        p.ready(move |p| p.complete(1u));

        assert_eq!(2u, rx.recv());
    }

    /*
     *
     * ===== Helper functions =====
     *
     */

    fn spawn<F: FnOnce() + Send>(f: F) {
        Thread::spawn(f).detach();
    }

    fn millis(ms: uint) -> Duration {
        Duration::milliseconds(ms as i64)
    }
}
