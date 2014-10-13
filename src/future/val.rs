//! A basic implementation of Future.
//!
//! As of now, the implementation is fairly naive, using a mutex to
//! handle synchronization. However, this will eventually be
//! re-implemented using lock free strategies once the API stabalizes.

use std::{fmt, mem};
use sync::{Arc, MutexCell, MutexCellGuard, CondVar};
use super::{Future, SyncFuture};

// TODO:
// * Consider renaming Completer -> Producer

pub fn future<T: Send>() -> (FutureVal<T>, Completer<T>) {
    let inner = FutureImpl::new();

    let f = FutureVal::new(inner.clone());
    let c = Completer::new(inner);

    (f, c)
}

pub struct FutureVal<T> {
    inner: FutureImpl<T>,
}

impl<T: Send> FutureVal<T> {
    /// Creates a new FutureVal with the given core
    #[inline]
    fn new(inner: FutureImpl<T>) -> FutureVal<T> {
        FutureVal { inner: inner }
    }
}

impl<T: Send> Future<T> for FutureVal<T> {
    #[inline]
    fn receive<F: FnOnce(T) + Send>(self, cb: F) {
        self.inner.receive(cb);
    }

    #[inline]
    fn cancel(self) {
        self.inner.cancel();
    }
}

impl<T: Send> SyncFuture<T> for FutureVal<T> {
    #[inline]
    fn take(self) -> T {
        self.inner.take()
    }
}

impl<T: fmt::Show> fmt::Show for FutureVal<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(fmt, "FutureVal"));
        Ok(())
    }
}

pub struct Completer<T> {
    inner: FutureImpl<T>,
}

impl<T: Send> Completer<T> {
    /// Creates a new Completer with the given core
    #[inline]
    fn new(inner: FutureImpl<T>) -> Completer<T> {
        Completer { inner: inner }
    }

    #[inline]
    pub fn complete(self, val: T) {
        self.inner.complete(val);
    }

    #[inline]
    pub fn fail(self, desc: &'static str) {
        self.inner.fail(desc);
    }
}

/*
 *
 * ===== Consumer interest future =====
 *
 */

impl<T: Send> Future<ConsumerState<T>> for Completer<T> {
    #[inline]
    fn receive<F: FnOnce(ConsumerState<T>) + Send>(self, cb: F) {
        self.inner.completer_receive(cb);
    }

    #[inline]
    fn cancel(self) {
        self.fail("canceled by producer");
    }
}

impl<T: Send> SyncFuture<ConsumerState<T>> for Completer<T> {
    fn take(self) -> ConsumerState<T> {
        self.inner.completer_take()
    }
}

pub enum ConsumerState<T> {
    Waiting(Completer<T>),
    Canceled,
}

impl<T> ConsumerState<T> {
    pub fn is_waiting(&self) -> bool {
        match *self {
            Waiting(..) => true,
            _ => false,
        }
    }

    pub fn is_canceled(&self) -> bool {
        match *self {
            Canceled => true,
            _ => false,
        }
    }

    pub fn unwrap(self) -> Completer<T> {
        match self {
            Waiting(v) => v,
            _ => fail!("called `ConsumerState::unwrap()` on a `Canceled` state"),
        }
    }
}

/*
 *
 * ===== Implementation details =====
 *
 */

struct FutureImpl<T> {
    core: Arc<MutexCell<Core<T>>>,
}

impl<T: Send> FutureImpl<T> {
    fn new() -> FutureImpl<T> {
        FutureImpl {
            core: Arc::new(MutexCell::new(Core::new()))
        }
    }

    fn receive<F: FnOnce(T) + Send>(self, cb: F) {
        // Acquire the lock
        let mut core = self.lock();

        // If the producer is currently waiting, notify it that the
        // consumer has indicated interest in the result.
        core = self.notify_completer(core, true);

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
        core.state = ConsumerWait(Callback(box cb));
    }

    fn take(self) -> T {
        // Acquire the lock
        let mut core = self.lock();

        // If the producer is currently waiting, notify it that the
        // consumer has indicated interest in the result.
        core = self.notify_completer(core, true);

        // Before the thread blocks, track that the consumer is waiting
        core.state = ConsumerWait(Sync);

        // Checking the value and waiting happens in a loop to handle
        // cases where the condition variable unblocks early for an
        // unknown reason (permitted by the pthread spec).
        loop {
            // Check if the value has been realized before blocking
            if let Some(val) = core.take_value() {
                return val;
            }

            // Wait on the condition variable
            core.wait(&core.condvar);
        }
    }

    fn cancel(self) {
        // Acquire the lock
        let mut core = self.lock();

        // If the producer is currently waiting, notify it that the
        // consumer has canceled the future.
        core = self.notify_completer(core, false);

        // Set the state to canceled
        core.state = ConsumerCanceled;
    }

    fn complete(self, val: T) {
        // Acquire the lock
        let mut core = self.lock();

        // Check if the consumer is waiting on the value, if so, it will
        // be notified that value is ready.
        if let ConsumerWait(strategy) = core.take_consumer_wait() {
            // Check the consumer wait strategy
            match strategy {
                // If the consumer is waiting with a callback, release
                // the lock and invoke the callback with the value.
                Callback(cb) => {
                    drop(core);
                    cb.call_once((val,));
                }
                // Otherwise, store the value on the future and signal
                // the consumer that the value is ready.
                Sync => {
                    core.put(val);
                    core.condvar.signal();
                }
            }

            return;
        }

        core.put(val);
    }

    fn fail(self, _desc: &'static str) {
        unimplemented!()
    }

    fn completer_receive<F: FnOnce(ConsumerState<T>) + Send>(self, cb: F) {
        let mut canceled;

        // Run the synchronized logic within a scope such that the lock
        // is released at the end of the scope.
        {
            // Acquire the lock
            let mut core = self.lock();

            // If the consumer has not registered an interest yet, save off
            // the callback for when it does and return;
            if core.state.is_pending() {
                core.state = CompleterWait(Callback(box cb));
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
            cb(Canceled);
        } else {
            // Invoke the callback with the completer (simply wrap the
            // FutureImpl instance)
            cb(Waiting(Completer::new(self)));
        }
    }

    fn completer_take(self) -> ConsumerState<T> {
        let mut canceled;

        // Run the synchronized logic within a scope such that the lock
        // is released at the end of the scope.
        {
            // Acquire the lock
            let mut core = self.lock();

            // If the consumer has not registered an interest yet, track
            // that the completer is about to block, then wait for the
            // signal.
            if core.state.is_pending() {
                core.state = CompleterWait(Sync);

                // Loop as long as the future remains in the completer wait
                // state.
                loop {
                    // Wait on the cond var
                    core.wait(&core.condvar);

                    // If the future state has changed, break out fo the
                    // loop.
                    if !core.state.is_completer_wait() {
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
            Canceled
        } else {
            // Return the completer (simply wrap the FutureImpl instance)
            Waiting(Completer::new(self))
        }
    }

    fn notify_completer<'a>(&'a self, mut core: LockedCore<'a, T>, interest: bool)
            -> LockedCore<'a, T> {

        // Run notification in a loop, the callback has the option to
        // re-register another receive callback, in which case it should
        // be immediately invoked.
        loop {
            if let CompleterWait(strategy) = core.take_completer_wait() {
                match strategy {
                    Callback(cb) => {
                        drop(core);

                        if interest {
                            cb.call_once((Waiting(Completer::new(self.clone())),));
                        } else {
                            cb.call_once((Canceled,));
                        }

                        core = self.lock();
                    }
                    Sync => core.condvar.signal(),
                }
            } else {
                break;
            }
        }

        core
    }

    #[inline]
    fn lock(&self) -> MutexCellGuard<Core<T>> {
        self.core.lock()
    }
}

impl<T: Send> Clone for FutureImpl<T> {
    fn clone(&self) -> FutureImpl<T> {
        FutureImpl { core: self.core.clone() }
    }
}

struct Core<T> {
    val: Option<T>,
    condvar: CondVar,
    state: State<T>,
}

type LockedCore<'a, T> = MutexCellGuard<'a, Core<T>>;

impl<T: Send> Core<T> {
    fn new() -> Core<T> {
        Core {
            val: None,
            condvar: CondVar::new(),
            state: Pending,
        }
    }

    fn put(&mut self, val: T) {
        assert!(self.val.is_none(), "future already completed");
        self.val = Some(val);
    }

    fn take_value(&mut self) -> Option<T> {
        mem::replace(&mut self.val, None)
    }

    fn take_consumer_wait(&mut self) -> State<T> {
        if self.state.is_consumer_wait() {
            mem::replace(&mut self.state, Pending)
        } else {
            Pending
        }
    }

    fn take_completer_wait(&mut self) -> State<T> {
        if self.state.is_completer_wait() {
            mem::replace(&mut self.state, Pending)
        } else {
            Pending
        }
    }
}

enum State<T> {
    Pending,
    ConsumerCanceled,
    ConsumerWait(WaitStrategy<T>),
    CompleterWait(WaitStrategy<ConsumerState<T>>),
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
            ConsumerCanceled => true,
            _ => false,
        }
    }

    fn is_consumer_wait(&self) -> bool {
        match *self {
            ConsumerWait(..) => true,
            _ => false,
        }
    }

    fn is_completer_wait(&self) -> bool {
        match *self {
            CompleterWait(..) => true,
            _ => false,
        }
    }
}

enum WaitStrategy<T> {
    Callback(Box<FnOnce<(T,), ()> + Send>),
    Sync,
}

#[cfg(test)]
mod test {
    use std::io::timer::sleep;
    use std::time::Duration;
    use sync::Arc;
    use sync::atomic::{AtomicBool, AtomicUint, Relaxed};
    use future::{Future, SyncFuture};
    use super::*;

    #[test]
    pub fn test_complete_before_take() {
        let (f, c) = future();

        spawn(proc() {
            c.complete("zomg");
        });

        sleep(Duration::milliseconds(50));
        assert_eq!(f.take(), "zomg");
    }

    #[test]
    pub fn test_complete_after_take() {
        let (f, c) = future();

        spawn(proc() {
            sleep(Duration::milliseconds(50));
            c.complete("zomg");
        });

        assert_eq!(f.take(), "zomg");
    }

    #[test]
    pub fn test_complete_before_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        spawn(proc() {
            c.complete("zomg");
        });

        sleep(Duration::milliseconds(50));
        f.receive(move |:v| tx.send(v));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_complete_after_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        spawn(proc() {
            sleep(Duration::milliseconds(50));
            c.complete("zomg");
        });

        f.receive(move |:v| tx.send(v));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_future_before_take() {
        let (f, c) = future::<&'static str>();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        c.receive(move |:c: ConsumerState<&'static str>| {
            assert!(w2.load(Relaxed));
            c.unwrap().complete("zomg");
        });

        w1.store(true, Relaxed);
        assert_eq!(f.take(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_future_after_take() {
        let (f, c) = future();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        spawn(proc() {
            sleep(Duration::milliseconds(50));

            c.receive(move |:c: ConsumerState<&'static str>| {
                assert!(w2.load(Relaxed));
                c.unwrap().complete("zomg");
            });
        });

        w1.store(true, Relaxed);
        assert_eq!(f.take(), "zomg");
    }

    #[test]
    pub fn test_receive_complete_before_consumer_receive() {
        let (f, c) = future();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        c.receive(move |:c: ConsumerState<&'static str>| {
            assert!(w2.load(Relaxed));
            c.unwrap().complete("zomg");
        });

        let (tx, rx) = channel();
        w1.store(true, Relaxed);

        f.receive(move |:msg| {
            assert_eq!("zomg", msg);
            tx.send("hi2u");
        });

        assert_eq!("hi2u", rx.recv());
    }

    #[test]
    pub fn test_receive_complete_after_consumer_receive() {
        let (f, c) = future();
        let w1 = Arc::new(AtomicBool::new(false));
        let w2 = w1.clone();

        spawn(proc() {
            sleep(Duration::milliseconds(50));

            c.receive(move |:c: ConsumerState<&'static str>| {
                assert!(w2.load(Relaxed));
                c.unwrap().complete("zomg");
            });
        });

        let (tx, rx) = channel();
        w1.store(true, Relaxed);

        f.receive(move |:msg| {
            assert_eq!("zomg", msg);
            tx.send("hi2u");
        });

        assert_eq!("hi2u", rx.recv());
    }

    #[test]
    pub fn test_take_complete_before_consumer_take() {
        let (f, c) = future();

        spawn(proc() {
            c.take().unwrap().complete("zomg");
        });

        sleep(Duration::milliseconds(50));
        assert_eq!("zomg", f.take());
    }

    #[test]
    pub fn test_take_complete_after_consumer_take() {
        let (f, c) = future();

        spawn(proc() {
            sleep(Duration::milliseconds(50));
            c.take().unwrap().complete("zomg");
        });

        assert_eq!("zomg", f.take());
    }

    #[test]
    pub fn test_take_complete_before_consumer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        spawn(proc() {
            c.take().unwrap().complete("zomg");
        });

        sleep(Duration::milliseconds(50));
        f.receive(move |:v| tx.send(v));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_take_complete_after_consumer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        spawn(proc() {
            sleep(Duration::milliseconds(50));
            c.take().unwrap().complete("zomg");
        });

        f.receive(move |:v| tx.send(v));
        assert_eq!(rx.recv(), "zomg");
    }

    // Utility method used below
    fn waiting(count: uint, d: Arc<AtomicUint>, c: Completer<&'static str>) {
        // Assert that the callback is not invoked recursively
        assert_eq!(0, d.fetch_add(1, Relaxed));

        if count == 5 {
            c.complete("done");
        } else {
            let d2 = d.clone();
            c.receive(move |:c: ConsumerState<&'static str>| {
                waiting(count + 1, d2, c.unwrap())
            });
        }

        d.fetch_sub(1, Relaxed);
    }

    #[test]
    pub fn test_completer_receive_when_consumer_cb_set() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();
        let depth = Arc::new(AtomicUint::new(0));

        waiting(0, depth, c);

        f.receive(move |:v| tx.send(v));
        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_completer_receive_when_consumer_waiting() {
        let (f, c) = future();
        let depth = Arc::new(AtomicUint::new(0));

        waiting(0, depth, c);

        sleep(Duration::milliseconds(50));
        assert_eq!(f.take(), "done");
    }

    #[test]
    pub fn test_completer_take_when_consumer_cb_set() {
        let (f, c) = future();
        let (tx, rx) = channel::<&'static str>();

        spawn(proc() {
            c.take().unwrap()
                .take().unwrap()
                .take().unwrap().complete("zomg");
        });

        sleep(Duration::milliseconds(50));
        f.receive(move |:v| tx.send(v));
        assert_eq!(rx.recv(), "zomg");
    }

    #[test]
    pub fn test_completer_take_when_consumer_waiting() {
        let (f, c) = future();

        spawn(proc() {
            c.take().unwrap()
                .take().unwrap()
                .take().unwrap().complete("zomg");
        });

        sleep(Duration::milliseconds(50));
        assert_eq!(f.take(), "zomg");
    }

    #[test]
    pub fn test_canceling_future_before_producer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel();

        f.cancel();

        c.receive(move |:s: ConsumerState<&'static str>| {
            assert!(s.is_canceled())
            tx.send("done");
        });

        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_canceling_future_before_producer_take() {
        let (f, c) = future::<uint>();

        f.cancel();

        assert!(c.take().is_canceled());
    }

    #[test]
    pub fn test_canceling_future_after_producer_receive() {
        let (f, c) = future();
        let (tx, rx) = channel();

        c.receive(move |:s: ConsumerState<&'static str>| {
            assert!(s.is_canceled());
            tx.send("done");
        });

        f.cancel();
        assert_eq!(rx.recv(), "done");
    }

    #[test]
    pub fn test_canceling_future_after_producer_take() {
        let (f, c) = future::<uint>();
        let (tx, rx) = channel();

        spawn(proc() {
            assert!(c.take().is_canceled());
            tx.send("done");
        });

        sleep(Duration::milliseconds(50));
        f.cancel();

        assert_eq!(rx.recv(), "done");
    }
}
