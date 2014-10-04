//! A basic implementation of Future.
//!
//! As of now, the implementation is fairly naive, using a mutex to
//! handle synchronization. However, this will eventually be
//! re-implemented using lock free strategies once the API stabalizes.

use std::mem;
use sync::{Arc, MutexCell, CondVar};
use super::{Future, SyncFuture};

pub fn future<T:Send>() -> (FutureVal<T>, Completer<T>) {
    let core = Arc::new(MutexCell::new(Core::new()));

    let f = FutureVal { core: core.clone() };
    let c = Completer { core: core };

    (f, c)
}

pub struct FutureVal<T> {
    core: Arc<MutexCell<Core<T>>>,
}

impl<T: Send> Future<T> for FutureVal<T> {
    fn receive<F: FnOnce<(T,), ()> + Send>(self, cb: F) {
        let mut l = self.core.lock();

        if let Some(v) = l.take_val() {
            drop(l); // Escape the mutex
            cb(v);
            return;
        }

        l.completion = Callback(box cb);
    }
}

impl<T: Send> SyncFuture<T> for FutureVal<T> {
    fn take(self) -> T {
        let mut l = self.core.lock();

        if let Some(v) = l.take_val() {
            return v;
        }

        l.completion = Wait;
        l.wait(&l.condvar);
        l.take_val().unwrap()
    }

    fn try_take(self) -> Result<T, FutureVal<T>> {
        unimplemented!();
    }
}

pub struct Completer<T> {
    core: Arc<MutexCell<Core<T>>>,
}

impl<T: Send> Completer<T> {
    pub fn complete(self, val: T) {
        let mut l = self.core.lock();

        if let Callback(cb) = l.take_callback() {
            drop(l);
            cb.call_once((val,));
            return;
        }

        l.put(val);

        if l.completion.is_wait() {
            println!("signaling cond var");
            l.condvar.signal();
        }
    }
}

struct Core<T> {
    val: Option<T>,
    condvar: CondVar,
    completion: Completion<T>,
}

impl<T:Send> Core<T> {
    fn new() -> Core<T> {
        Core {
            val: None,
            condvar: CondVar::new(),
            completion: Pending,
        }
    }

    fn put(&mut self, val: T) {
        assert!(self.val.is_none(), "future already completed");
        self.val = Some(val);
    }

    fn take_val(&mut self) -> Option<T> {
        mem::replace(&mut self.val, None)
    }

    fn take_callback(&mut self) -> Completion<T> {
        if self.completion.is_callback() {
            mem::replace(&mut self.completion, Pending)
        } else {
            Pending
        }
    }
}

enum Completion<T> {
    Pending,
    Wait,
    Callback(Box<FnOnce<(T,),()> + Send>),
}

impl<T: Send> Completion<T> {
    fn is_wait(&self) -> bool {
        match *self {
            Wait => true,
            _ => false,
        }
    }

    fn is_callback(&self) -> bool {
        match *self {
            Callback(..) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod test {
    use std::io::timer::sleep;
    use std::time::Duration;
    use super::*;
    use future::{Future, SyncFuture};

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
}
