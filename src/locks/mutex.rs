//! Implements mutex and condition variable primitives backed by the
//! equivalent pthread primitives.

use core::cell::UnsafeCell;
use time::get_time;
use ffi;

/// A wrapper type which provides synchronized access to the underlying data, of
/// type `T`. A mutex always provides exclusive access, and concurrent requests
/// will block while the mutex is already locked.
pub struct MutexCell<T> {
    lock: Mutex,
    data: UnsafeCell<T>,
}

impl<T> MutexCell<T> {
    pub fn new(data: T) -> MutexCell<T> {
        MutexCell {
            lock: Mutex::new(),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock<'a>(&'a self) -> MutexCellGuard<'a, T> {
        let lock = self.lock.lock();

        MutexCellGuard {
            __lock: lock,
            __data: unsafe { self.get_mut() }
        }
    }

    pub fn try_lock<'a>(&'a self) -> Option<MutexCellGuard<'a, T>> {
        if let Some(lock) = self.lock.try_lock() {
            return Some(MutexCellGuard {
                __lock: lock,
                __data: unsafe { self.get_mut() }
            });
        }

        None
    }

    pub unsafe fn get(&self) -> &T {
        &*self.data.get()
    }

    unsafe fn get_mut(&self) -> &mut T {
        &mut *self.data.get()
    }
}

pub struct MutexCellGuard<'a, T:'a> {
    // Underscores are used to not conflict with properties on T
    __lock: MutexGuard<'a>,
    __data: &'a mut T,
}

impl<'a, T> MutexCellGuard<'a, T> {
    pub fn wait(&self, cond: &CondVar) {
        self.__lock.wait(cond);
    }

    pub fn timed_wait(&self, cond: &CondVar, ms: uint) {
        self.__lock.timed_wait(cond, ms);
    }
}

impl<'a, T> Deref<T> for MutexCellGuard<'a, T> {
    fn deref(&self) -> &T {
        self.__data
    }
}

impl<'a, T> DerefMut<T> for MutexCellGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.__data
    }
}

pub struct Mutex {
    native: ffi::pthread_mutex_t,
}

impl Mutex {
    pub fn new() -> Mutex {
        Mutex { native: ffi::PTHREAD_MUTEX_INITIALIZER }
    }

    pub fn lock<'a>(&'a self) -> MutexGuard<'a> {
        unsafe {
            let res = ffi::pthread_mutex_lock(
                &self.native as *const ffi::pthread_mutex_t);

            if res < 0 {
                fail!("unexpected internal mutex state");
            }
        }

        MutexGuard { native: &self.native }
    }

    pub fn try_lock<'a>(&'a self) -> Option<MutexGuard<'a>> {
        unsafe {
            let res = ffi::pthread_mutex_trylock(
                &self.native as *const ffi::pthread_mutex_t);

            if res == 0 {
                Some(MutexGuard { native: &self.native })
            } else if res == ffi::EBUSY {
                return None
            } else {
                fail!("unexpected internal mutex state");
            }
        }
    }
}

impl Drop for Mutex {
    fn drop(&mut self) {
        unsafe {
            let res = ffi::pthread_mutex_destroy(
                &self.native as *const ffi::pthread_mutex_t);

            if res < 0 {
                fail!("unexpected internal mutex state");
            }
        }
    }
}

pub struct MutexGuard<'a> {
    native: &'a ffi::pthread_mutex_t,
}

impl<'a> MutexGuard<'a> {
    pub fn wait(&self, cond: &CondVar) {
        cond.wait(self.native);
    }

    pub fn timed_wait(&self, cond: &CondVar, ms: uint) {
        cond.timed_wait(self.native, ms);
    }
}

#[unsafe_destructor]
impl<'a> Drop for MutexGuard<'a> {
    fn drop(&mut self) {
        unsafe {
            let res = ffi::pthread_mutex_unlock(
                self.native as *const ffi::pthread_mutex_t);

            if res < 0 {
                fail!("unexpected internal mutex state");
            }
        }
    }
}

pub struct CondVar {
    native: ffi::pthread_cond_t,
}

impl CondVar {
    pub fn new() -> CondVar {
        CondVar { native: ffi::PTHREAD_COND_INITIALIZER }
    }

    pub fn signal(&self) {
        unsafe {
            let res = ffi::pthread_cond_signal(
                &self.native as *const ffi::pthread_cond_t);

            if res < 0 {
                fail!("unexpected internal condition variable state");
            }
        }
    }

    fn wait(&self, lock: &ffi::pthread_mutex_t) {
        unsafe {
            let res = ffi::pthread_cond_wait(
                &self.native as *const ffi::pthread_cond_t,
                lock as *const ffi::pthread_mutex_t);

            if res < 0 {
                fail!("unexpected internal condition variable state");
            }
        }
    }

    fn timed_wait(&self, lock: &ffi::pthread_mutex_t, ms: uint) {
        let ts = ms_to_abs(ms);

        unsafe {
            let res = ffi::pthread_cond_timedwait(
                &self.native as *const ffi::pthread_cond_t,
                lock as *const ffi::pthread_mutex_t,
                &ts as *const ffi::timespec);

            if res == 0 || res == ffi::ETIMEDOUT {
                return;
            }

            fail!("unexpected internal condition variable state");
        }
    }
}

impl Drop for CondVar {
    fn drop(&mut self) {
        unsafe {
            let res = ffi::pthread_cond_destroy(
                &self.native as *const ffi::pthread_cond_t);

            if res < 0 {
                fail!("unexpected internal condition variable state");
            }
        }
    }
}

static MAX_WAIT: uint = 1_000_000;
static MS_PER_SEC: uint = 1_000;
static NANOS_PER_MS: uint = 1_000_000;
static NANOS_PER_SEC: uint = 1_000_000_000;

fn ms_to_abs(ms: uint) -> ffi::timespec {
    use libc::{c_long, time_t};

    let mut ts = get_time();
    let mut sec = ms / MS_PER_SEC;
    let nsec = (ms & MS_PER_SEC) + NANOS_PER_MS;

    if sec > MAX_WAIT {
        sec = MAX_WAIT;
    }

    ts.sec += sec as i64;
    ts.nsec += nsec as i32;

    if ts.nsec >= NANOS_PER_SEC as i32 {
        ts.sec += 1;
        ts.nsec -= NANOS_PER_SEC as i32;
    }

    ffi::timespec {
        tv_sec: ts.sec as time_t,
        tv_nsec: ts.nsec as c_long,
    }
}
