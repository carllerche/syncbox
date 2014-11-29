//! Efficient thread block / unblock semantics.
//!
//! This is a placeholder implementation until something (hopefully better)
//! lands in Rust's stdlib.
use std::time::Duration;
use ffi;
use locks::{Mutex, CondVar};
use std::sync::Arc;
use std::sync::atomic::{AtomicUint, Ordering, Relaxed};

pub fn unparker() -> Unparker {
    with_parker(|p| Unparker { parker: p.clone() })
}

pub fn park(order: Ordering) {
    with_parker(|p| p.park_ms(0, order));
}

pub fn park_timed(timeout: Duration, order: Ordering) {
    use std::uint;
    use std::num::NumCast;

    with_parker(|p| {
        let ms = NumCast::from(timeout.num_milliseconds());
        p.park_ms(ms.unwrap_or(uint::MAX), order)
    });
}

/// Handle to a thread's parker
pub struct Unparker {
    parker: Arc<Parker>,
}

impl Unparker {
    pub fn unpark(&self, order: Ordering) {
        self.parker.unpark(order);
    }
}

// The thread is not parked nor signaled for unpark
const INITAL: uint = 0;
// The thread is currently parked
const PARKED: uint = 1;
// The thread has been signaled for unpark
const UNPARK: uint = 2;

struct Parker {
    // 1 for unconsumed unpark flag, 0 otherwise
    state: AtomicUint,
    mutex: Mutex,
    condvar: CondVar,
}

impl Parker {
    fn new() -> Parker {
        Parker {
            state: AtomicUint::new(INITAL),
            mutex: Mutex::new(),
            condvar: CondVar::new(),
        }
    }

    fn park_ms(&self, timeout_ms: uint, order: Ordering) {
        let mut old;

        old = self.state.compare_and_swap(UNPARK, PARKED, order);

        // Fast path, there already is a pending unpark
        if old == UNPARK {
            return;
        }

        let lock = match self.mutex.try_lock() {
            Some(l) => l,
            // Lock could not be acquired, just return assuming that caller
            // will retry
            None => return,
        };

        // In critical section, check the state again before blocking
        //
        // TODO: I'm not 100% sure how pthread locks interact w/ the
        // C++11 memory model. Pthread locks have a very ordering
        // requirement, but how does it interact with another thread
        // that issues an Acquire or a Release?
        //
        // For now, use the specified order
        old = self.state.swap(PARKED, order);

        if old == UNPARK {
            return;
        }

        // Even if the wait fails, assume that it has been consumed, update the
        // state and carry on.
        if timeout_ms == 0 {
            lock.wait(&self.condvar);
        } else {
            lock.timed_wait(&self.condvar, timeout_ms);
        };

        // Store the new state
        self.state.store(INITAL, Relaxed);
    }

    pub fn unpark(&self, order: Ordering) {
        if self.state.swap(UNPARK, order) == PARKED {
            // If there are no threads currently parked, then signaling will
            // have no effect. The lock is to ensure that if the parking thread
            // has entered the critical section, it will have reached the wait
            // point before the signal is fired.
            let _lock = self.mutex.lock();
            self.condvar.signal();
        }
    }
}

fn with_parker<T>(cb: |&Arc<Parker>| -> T) -> T {
    use std::mem;
    use libc::c_void;

    static mut INIT: ffi::pthread_once_t = ffi::PTHREAD_ONCE_INIT;
    static mut TLS: ffi::pthread_key_t = 0;

    // Executed exactly once on first call
    extern "C" fn init_tls_key() {
        unsafe {
            // Initialize the TLS key
            ffi::pthread_key_create(
                &mut TLS as *mut ffi::pthread_key_t,
                free_park);
        }
    }

    extern "C" fn free_park(ptr: *mut c_void) {
        unsafe {
            let handle: Arc<Parker> = mem::transmute(ptr);
            drop(handle);
        }
    }

    unsafe {
        ffi::pthread_once(&mut INIT as *mut ffi::pthread_once_t, init_tls_key);

        let mut ptr = ffi::pthread_getspecific(TLS);

        if ptr.is_null() {
            ptr = mem::transmute(Arc::new(Parker::new()));
            ffi::pthread_setspecific(TLS, ptr as *const c_void);
        }

        let park: Arc<Parker> = mem::transmute(ptr);

        let ret = cb(&park);
        mem::forget(park);
        ret
    }
}
