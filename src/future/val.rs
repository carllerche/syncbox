//! A basic implementation of Future.
//!
//! As of now, the implementation is fairly naive. A single mutex is used to
//! handle synchronization and the implementation generally doesn't worry that
//! much about performance at the moment. The goal currently is to figure out
//! the API and then worry about performance. Eventually FutureVal will be
//! re-implementing using lock-free strategies and focus on performance.

// TODO:
// - FutureVal::take_timed()
// - Interest::cancel()
// - Deref
// - clone & producer take / receive

use super::{
    Cancel,
    Future,
    SyncFuture,
    FutureResult,
    FutureError,
};
use super::FutureErrorKind::*;
use self::State::*;
use self::WaitStrategy::*;

use std::{fmt, mem};
use std::time::Duration;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::Relaxed;
use std::thread::Thread;

pub fn future<T: Send>() -> (FutureVal<T>, Producer<T>) {
    let inner = FutureImpl::new();

    let f = FutureVal::new(inner.clone());
    let c = Producer::new(inner);

    (f, c)
}

pub struct FutureVal<T> {
    inner: Option<FutureImpl<T>>,
}

impl<T: Send> FutureVal<T> {
    /// Creates a new FutureVal with the given core
    #[inline]
    fn new(inner: FutureImpl<T>) -> FutureVal<T> {
        FutureVal { inner: Some(inner) }
    }
}

impl<T: Send> Future<T, Interest> for FutureVal<T> {
    #[inline]
    fn receive<F: FnOnce(FutureResult<T>) + Send>(mut self, cb: F) -> Interest {
        self.inner.take().unwrap().receive(cb);
        Interest
    }
}

impl<T: Send> SyncFuture<T> for FutureVal<T> {
    #[inline]
    fn take(mut self) -> FutureResult<T> {
        self.inner.take().unwrap().take()
    }

    fn take_timed(self, _timeout: Duration) -> FutureResult<T> {
        unimplemented!();
    }
}

impl<T: Send> Cancel for FutureVal<T> {
    #[inline]
    fn cancel(mut self) {
        self.inner.take().unwrap().cancel();
    }
}

impl<T: Send + Clone> Clone for FutureVal<T> {
    fn clone(&self) -> FutureVal<T> {
        // Used to clone the value
        fn cloner<T: Clone>(val: &FutureResult<T>) -> FutureResult<T> {
            match *val {
                Ok(ref v) => Ok(v.clone()),
                Err(e) => Err(e),
            }
        }

        if let Some(ref inner) = self.inner {
            inner.consumer_clone(cloner);

            return FutureVal { inner: Some(inner.clone()) };
        }

        panic!("[BUG] attempting to clone a consumed future value");
    }
}

#[unsafe_destructor]
impl<T: Send> Drop for FutureVal<T> {
    fn drop(&mut self) {
        match self.inner.take() {
            Some(inner) => inner.cancel(),
            None => {}
        }
    }
}

impl<T: fmt::Show> fmt::Show for FutureVal<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(fmt, "FutureVal"));
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
    inner: Option<FutureImpl<T>>,
}

impl<T: Send> Producer<T> {
    /// Creates a new Producer with the given core
    #[inline]
    fn new(inner: FutureImpl<T>) -> Producer<T> {
        Producer { inner: Some(inner) }
    }

    #[inline]
    pub fn complete(mut self, val: T) {
        self.inner.take().unwrap().complete(Ok(val));
    }

    #[inline]
    pub fn fail(mut self, desc: &'static str) {
        let inner = self.inner.take().unwrap();
        inner.complete(Err(FutureError {
            kind: ExecutionError,
            desc: desc,
        }));
    }
}

#[unsafe_destructor]
impl<T: Send> Drop for Producer<T> {
    fn drop(&mut self) {
        match self.inner.take() {
            Some(inner) => {
                inner.complete(Err(FutureError {
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

impl<T: Send> Future<Producer<T>, ProducerInterest> for Producer<T> {
    #[inline]
    fn receive<F: FnOnce(FutureResult<Producer<T>>) + Send>(mut self, cb: F) -> ProducerInterest {
        let inner = self.inner.take().unwrap();
        inner.producer_receive(cb);
        ProducerInterest
    }
}

impl<T: Send> SyncFuture<Producer<T>> for Producer<T> {
    fn take(mut self) -> FutureResult<Producer<T>> {
        let inner = self.inner.take().unwrap();
        inner.producer_take()
    }

    fn take_timed(self, _timeout: Duration) -> FutureResult<Producer<T>> {
        unimplemented!()
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
// FutureImpl handles the implementation of FutureVal & Producer. It manages
// completing the value for the primary FutureVal instance as well as any
// clones.
//
// FutureImpl consists of a state machine that manages the internal state of
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
// If type T implements Clone then FutureVal can be cloned. FutureImpl will
// track the number of interested consumers. Then, once the future has been
// realized and consumers start taking the result, FutureImpl will clone the
// value then. For the last consumer, the value will be moved out.
//
// The future will transition to complete only once all the interested
// consumers have received the value.
//
// The future will transition to canceled only once all the interested
// consumers have issued a cancel.

struct FutureImpl<T> {
    core: Arc<Mutex<Core<T>>>,
}

impl<T: Send> FutureImpl<T> {
    fn new() -> FutureImpl<T> {
        FutureImpl {
            core: Arc::new(Mutex::new(Core::new()))
        }
    }

    fn receive<F: FnOnce(FutureResult<T>) + Send>(&self, cb: F) {
        // Acquire the lock
        let mut core = self.lock();

        // If the producer is currently waiting, notify it that the
        // consumer has indicated interest in the result.
        core = self.producer_notify(core, true);

        // If the future has already been realized, move the value out
        // of the core so that it can be sent to the supplied callback.
        if let Some(val) = core.take_value() {
            // Drop the lock before invoking the callback (prevent
            // deadlocks).
            drop(core);
            cb(val);
            return;
        }

        // The future's value has not yet been realized. Save off the
        // callback and mark the consumer as waiting for the value. When
        // the value is available, the calback will be invoked with it.
        let i: Box<Invoke<(FutureResult<T>,), ()> + Send> = box cb;
        core.push_consumer_wait(Callback(i));
    }

    fn take(&self) -> FutureResult<T> {
        // Acquire the lock
        let mut core = self.lock();

        // If the producer is currently waiting, notify it that the
        // consumer has indicated interest in the result.
        core = self.producer_notify(core, true);

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

            // Set the state to canceled
            core.state = Canceled;
        }
    }

    fn complete(self, val: FutureResult<T>) {
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
                        // Grab the value (or a clone)
                        let val = core.take_value().expect("[BUG] WAT the value was just set!");
                        // Relase the lock
                        drop(core);
                        // Invoke the callback
                        cb.invoke((val,));
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

    fn producer_receive<F: FnOnce(FutureResult<Producer<T>>) + Send>(self, cb: F) {
        let mut canceled;

        // Run the synchronized logic within a scope such that the lock
        // is released at the end of the scope.
        {
            // Acquire the lock
            let mut core = self.lock();

            // If the consumer has not registered an interest yet, save off
            // the callback for when it does and return;
            if core.state.is_pending() {
                core.state = ProducerWait(Callback(box cb));
                return;
            }

            // Check if the consumer canceled interest
            canceled = core.state.is_canceled();

            // The consumer has registered an interest in the value. Release
            // the lock then invoke the callback. This allows the callback
            // to run outside of the lock preventing deadlocks.
            drop(core);
        }

        if canceled {
            // Invoke the callback with the canceled state
            cb(Err(cancelation()));
        } else {
            // Invoke the callback with the producer (simply wrap the
            // FutureImpl instance)
            cb(Ok(Producer::new(self)));
        }
    }

    fn producer_take(self) -> FutureResult<Producer<T>> {
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
            // Return the producer (simply wrap the FutureImpl instance)
            Ok(Producer::new(self))
        }
    }

    fn producer_notify<'a>(&'a self, mut core: LockedCore<'a, T>, interest: bool)
            -> LockedCore<'a, T> {

        // Run notification in a loop, the callback has the option to
        // re-register another receive callback, in which case it should
        // be immediately invoked.
        loop {
            if let ProducerWait(strategy) = core.take_producer_wait() {
                match strategy {
                    Callback(cb) => {
                        drop(core);

                        if interest {
                            cb.invoke((Ok(Producer::new(self.clone())),));
                        } else {
                            cb.invoke((Err(cancelation()),));
                        }

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

impl<T: Send> Clone for FutureImpl<T> {
    fn clone(&self) -> FutureImpl<T> {
        FutureImpl { core: self.core.clone() }
    }
}

struct Core<T> {
    // The realized value
    val: Option<FutureResult<T>>,
    // State of the future
    state: State<T>,
    // Number of clones
    clones: uint,
    // Clone function
    cloner: Option<Cloner<T>>,
}

type LockedCore<'a, T> = MutexGuard<'a, Core<T>>;
type Cloner<T> = fn(&FutureResult<T>) -> FutureResult<T>;

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

    fn put(&mut self, val: FutureResult<T>) {
        assert!(self.val.is_none(), "future already completed");
        self.val = Some(val);
    }

    fn take_value(&mut self) -> Option<FutureResult<T>> {
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

    fn push_consumer_wait(&mut self, strategy: WaitStrategy<T>) -> uint {
        if let ConsumerWait(ref mut vec) = self.state {
            let ret = vec.len();
            vec.push(Some(strategy));
            ret
        } else {
            self.state = ConsumerWait(vec![Some(strategy)]);
            0
        }
    }

    fn take_producer_wait(&mut self) -> State<T> {
        if self.state.is_producer_wait() {
            mem::replace(&mut self.state, Pending)
        } else {
            Pending
        }
    }
}

enum State<T> {
    Pending,
    ConsumerWait(Vec<Option<WaitStrategy<T>>>),
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
}

impl<T> fmt::Show for State<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", match *self {
            Pending => "Pending",
            Complete => "Complete",
            Canceled => "Canceled",
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
    Callback(Box<Invoke<(FutureResult<T>,), ()> + Send>),
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

fn cancelation() -> FutureError {
    FutureError {
        kind: CancelationError,
        desc: "future was canceled by consumer",
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use future::{Future, SyncFuture, Cancel, FutureResult};
    use std::io::timer::sleep;
    use std::time::Duration;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUint, Relaxed};
    use std::thread::Thread;

    #[test]
    pub fn test_complete_before_take() {
        let (f, c) = future();

        Thread::spawn(move || {
            c.complete("zomg");
        }).detach();

        sleep(millis(50));
        assert_eq!(f.take().unwrap(), "zomg");
    }

    #[test]
    pub fn test_complete_after_take() {
        let (f, c) = future();

        Thread::spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        }).detach();

        assert_eq!(f.take().unwrap(), "zomg");
    }

    #[test]
    pub fn test_complete_before_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        Thread::spawn(move || {
            c.complete("zomg");
        }).detach();

        sleep(millis(50));
        f.receive(move |:v: FutureResult<&'static str>| tx.send(v.unwrap()));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_complete_after_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        Thread::spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        }).detach();

        f.receive(move |:v: FutureResult<&'static str>| tx.send(v.unwrap()));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_future_before_take() {
        let (f, c) = future::<&'static str>();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        c.receive(move |:c: FutureResult<Producer<&'static str>>| {
            assert!(w2.load(Relaxed));
            c.unwrap().complete("zomg");
        });

        w1.store(true, Relaxed);
        assert_eq!(f.take().unwrap(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_future_after_take() {
        let (f, c) = future();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        Thread::spawn(move || {
            sleep(millis(50));

            c.receive(move |:c: FutureResult<Producer<&'static str>>| {
                assert!(w2.load(Relaxed));
                c.unwrap().complete("zomg");
            });
        }).detach();

        w1.store(true, Relaxed);
        assert_eq!(f.take().unwrap(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_before_consumer_receive() {
        let (f, c) = future();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        c.receive(move |:c: FutureResult<Producer<&'static str>>| {
            assert!(w2.load(Relaxed));
            c.unwrap().complete("zomg");
        });

        let (tx, rx) = channel();
        w1.store(true, Relaxed);

        f.receive(move |:msg: FutureResult<&'static str>| {
            assert_eq!("zomg", msg.unwrap());
            tx.send("hi2u");
        });

        assert_eq!("hi2u", rx.recv());
    }

    #[test]
    pub fn test_receive_complete_after_consumer_receive() {
        let (f, c) = future();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        Thread::spawn(move || {
            sleep(millis(50));

            c.receive(move |:c: FutureResult<Producer<&'static str>>| {
                assert!(w2.load(Relaxed));
                c.unwrap().complete("zomg");
            });
        }).detach();

        let (tx, rx) = channel();
        w1.store(true, Relaxed);

        f.receive(move |:msg: FutureResult<&'static str>| {
            assert_eq!("zomg", msg.unwrap());
            tx.send("hi2u");
        });

        assert_eq!("hi2u", rx.recv());
    }

    #[test]
    pub fn test_take_complete_before_consumer_take() {
        let (f, c) = future();

        Thread::spawn(move || {
            c.take().unwrap().complete("zomg");
        }).detach();

        sleep(millis(50));
        assert_eq!("zomg", f.take().unwrap());
    }

    #[test]
    pub fn test_take_complete_after_consumer_take() {
        let (f, c) = future();

        Thread::spawn(move || {
            sleep(millis(50));
            c.take().unwrap().complete("zomg");
        }).detach();

        assert_eq!("zomg", f.take().unwrap());
    }

    #[test]
    pub fn test_take_complete_before_consumer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        Thread::spawn(move || {
            c.take().unwrap().complete("zomg");
        }).detach();

        sleep(millis(50));
        f.receive(move |:v: FutureResult<&'static str>| {
            tx.send(v.unwrap())
        });
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_take_complete_after_consumer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        Thread::spawn(move || {
            sleep(millis(50));
            c.take().unwrap().complete("zomg");
        }).detach();

        f.receive(move |:v: FutureResult<&'static str>| {
            tx.send(v.unwrap())
        });
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
            c.receive(move |:c: FutureResult<Producer<&'static str>>| {
                waiting(count + 1, d2, c.unwrap())
            });
        }

        d.fetch_sub(1, Relaxed);
    }

    #[test]
    pub fn test_producer_receive_when_consumer_cb_set() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();
        let depth = Arc::new(AtomicUint::new(0));

        waiting(0, depth, c);

        f.receive(move |:v: FutureResult<&'static str>| {
            tx.send(v.unwrap())
        });

        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_producer_receive_when_consumer_waiting() {
        let (f, c) = future();
        let depth = Arc::new(AtomicUint::new(0));

        waiting(0, depth, c);

        sleep(millis(50));
        assert_eq!(f.take().unwrap(), "done");
    }

    #[test]
    pub fn test_producer_take_when_consumer_cb_set() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        Thread::spawn(move || {
            c.take().unwrap()
                .take().unwrap()
                .take().unwrap().complete("zomg");
        }).detach();

        sleep(millis(50));

        f.receive(move |:v: FutureResult<&'static str>| {
            tx.send(v.unwrap())
        });

        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_producer_take_when_consumer_waiting() {
        let (f, c) = future();

        Thread::spawn(move || {
            c.take().unwrap()
                .take().unwrap()
                .take().unwrap().complete("zomg");
        }).detach();

        sleep(millis(50));
        assert_eq!(f.take().unwrap(), "zomg");
    }

    #[test]
    pub fn test_canceling_future_before_producer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel();

        f.cancel();

        c.receive(move |:s: FutureResult<Producer<&'static str>>| {
            assert!(s.is_err());
            tx.send("done");
        });

        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_canceling_future_before_producer_take() {
        let (f, c) = future::<uint>();

        f.cancel();

        assert!(c.take().is_err());
    }

    #[test]
    pub fn test_canceling_future_after_producer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel();

        c.receive(move |:s: FutureResult<Producer<&'static str>>| {
            assert!(s.is_err());
            tx.send("done");
        });

        f.cancel();
        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_canceling_future_after_producer_take() {
        let (f, c) = future::<uint>();
        let (tx, rx) = channel();

        Thread::spawn(move || {
            assert!(c.take().is_err());
            tx.send("done");
        }).detach();

        sleep(millis(50));
        f.cancel();

        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_dropping_val_signals_cancelation() {
        let (f, c) = future::<uint>();
        let (tx, rx) = channel();

        Thread::spawn(move || {
            assert!(c.take().is_err());
            tx.send("done");
        }).detach();

        drop(f);
        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_producer_fail_before_consumer_take() {
        let (f, c) = future::<uint>();

        c.fail("nope");

        let err = f.take().unwrap_err();
        assert_eq!(err.desc, "nope");
        assert!(err.is_execution_error());
    }

    #[test]
    pub fn test_producer_fail_consumer_receive() {
        let (f, c) = future::<uint>();

        Thread::spawn(move || {
            sleep(millis(50));
            c.fail("nope");
        }).detach();

        let err = f.take().unwrap_err();
        assert_eq!(err.desc, "nope");
        assert!(err.is_execution_error());
    }

    #[test]
    pub fn test_producer_drops_before_consumer_take() {
        let (f, c) = future::<uint>();

        drop(c);

        let err = f.take().unwrap_err();
        assert_eq!(err.desc, "producer dropped out of scope without completing");
        assert!(err.is_execution_error());
    }

    #[test]
    pub fn test_producer_drops_after_consumer_take() {
        let (f, c) = future::<uint>();

        Thread::spawn(move || {
            sleep(millis(50));
            drop(c);
        }).detach();

        let err = f.take().unwrap_err();
        assert_eq!(err.desc, "producer dropped out of scope without completing");
        assert!(err.is_execution_error());
    }

    // ======= Cloning futures =======

    #[test]
    pub fn test_cloning_clone_complete_take_both() {
        let (f1, c) = future();
        let f2 = f1.clone();

        c.complete("zomg");

        assert_eq!(f1.take().unwrap(), "zomg");
        assert_eq!(f2.take().unwrap(), "zomg");
    }

    #[test]
    pub fn test_cloning_clone_take_both_complete() {
        let (f1, c) = future();
        let f2 = f1.clone();

        let (tx, rx) = channel();

        Thread::spawn(move || tx.send(f2.take().unwrap())).detach();
        Thread::spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        }).detach();

        assert_eq!(f1.take().unwrap(), "zomg");
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_cloning_clone_take_complete_take() {
        let (f1, c) = future();
        let f2 = f1.clone();

        Thread::spawn(move || {
            sleep(millis(50));
            c.complete("zomg");
        }).detach();

        assert_eq!(f1.take().unwrap(), "zomg");
        assert_eq!(f2.take().unwrap(), "zomg");
    }

    #[test]
    pub fn test_cloning_clone_complete_receive_both() {
        let (f1, c) = future();
        let f2 = f1.clone();

        c.complete("zomg");

        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        f1.receive(move |:v: FutureResult<&'static str>| tx1.send(v.unwrap()));
        f2.receive(move |:v: FutureResult<&'static str>| tx2.send(v.unwrap()));

        assert_eq!(rx1.recv(), "zomg");
        assert_eq!(rx2.recv(), "zomg");
    }

    #[test]
    pub fn test_cloning_receive_both_complete() {
        let (f1, c) = future();
        let f2 = f1.clone();

        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        f1.receive(move |:v: FutureResult<&'static str>| tx1.send(v.unwrap()));
        f2.receive(move |:v: FutureResult<&'static str>| tx2.send(v.unwrap()));

        c.complete("zomg");

        assert_eq!(rx1.recv(), "zomg");
        assert_eq!(rx2.recv(), "zomg");
    }

    #[test]
    pub fn test_cloning_receive_complete_receive() {
        let (f1, c) = future();
        let f2 = f1.clone();

        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        f1.receive(move |:v: FutureResult<&'static str>| tx1.send(v.unwrap()));
        c.complete("zomg");
        f2.receive(move |:v: FutureResult<&'static str>| tx2.send(v.unwrap()));


        assert_eq!(rx1.recv(), "zomg");
        assert_eq!(rx2.recv(), "zomg");
    }

    // ======= Util fns =======

    fn millis(ms: uint) -> Duration {
        Duration::milliseconds(ms as i64)
    }
}
