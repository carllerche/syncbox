#![allow(non_camel_case_types)]

use libc::{c_int, c_void};
pub use libc::timespec;
pub use libc::consts::os::posix88::{EBUSY, ETIMEDOUT};
pub use self::os::{
    pthread_once_t,
    pthread_key_t,
    PTHREAD_ONCE_INIT
};
pub use self::os::arch::{
    pthread_mutex_t,
    pthread_cond_t,
    PTHREAD_MUTEX_INITIALIZER,
    PTHREAD_COND_INITIALIZER,
};

#[cfg(target_os = "macos")]
mod os {
    use libc::c_ulong;

    #[repr(C)]
    pub struct pthread_once_t {
        _sig: u64,
        _opaque: [u8, ..4],
    }

    // Thread local storage handle
    pub type pthread_key_t = c_ulong;

    pub const PTHREAD_ONCE_INIT: pthread_once_t =
        pthread_once_t {
            _sig: 0x30B1BCBA,
            _opaque: [0, 0, 0, 0],
        };

    #[cfg(target_arch = "x86_64")]
    pub mod arch {
        #[repr(C)]
        pub struct pthread_mutex_t {
            _sig: u64,
            _opaque: [u64, ..7],
        }

        pub const PTHREAD_MUTEX_INITIALIZER: pthread_mutex_t =
            pthread_mutex_t {
                _sig: 0x32AAABA7,
                _opaque: [0, 0, 0, 0, 0, 0, 0],
            };

        #[repr(C)]
        pub struct pthread_cond_t {
            _sig: u64,
            _opaque: [u64, ..5]
        }

        pub const PTHREAD_COND_INITIALIZER: pthread_cond_t =
            pthread_cond_t {
                _sig: 0x3CB0B1BB,
                _opaque: [0, 0, 0, 0, 0],
            };
    }
}

#[cfg(target_os = "linux")]
mod os {
    use libc::{c_int, c_uint};

    pub type pthread_once_t = c_int;
    pub type pthread_key_t = c_uint;

    pub const PTHREAD_ONCE_INIT: pthread_once_t = 0;

    #[cfg(target_arch = "x86_64")]
    pub mod arch {
        #[repr(C)]
        pub struct pthread_mutex_t {
            _data: [u64, ..5u],
        }

        pub const PTHREAD_MUTEX_INITIALIZER: pthread_mutex_t =
            pthread_mutex_t {
                _data: [0, 0, 0, 0, 0],
            };

        #[repr(C)]
        pub struct pthread_cond_t {
            _data: [u64, ..6u],
        }

        pub const PTHREAD_COND_INITIALIZER: pthread_cond_t =
            pthread_cond_t {
                _data: [0, 0, 0, 0, 0, 0],
            };
    }
}

#[link(name = "pthread")]
extern {

    // == Thread routines ==
    pub fn pthread_once(
        once_control: *mut pthread_once_t,
        init_routine: extern "C" fn()) -> c_int;

    // == Mutex routines ==
    pub fn pthread_mutex_lock(mutex: *const pthread_mutex_t) -> c_int;
    pub fn pthread_mutex_trylock(mutex: *const pthread_mutex_t) -> c_int;
    pub fn pthread_mutex_unlock(mutex: *const pthread_mutex_t) -> c_int;

    // == Condition variable routines ==
    pub fn pthread_cond_wait(
        cond: *const pthread_cond_t,
        mutex: *const pthread_mutex_t) -> c_int;

    pub fn pthread_cond_timedwait(
        cond: *const pthread_cond_t,
        mutex: *const pthread_mutex_t,
        time: *const timespec) -> c_int;

    pub fn pthread_cond_signal(cond: *const pthread_cond_t) -> c_int;

    pub fn pthread_mutex_destroy(mutex: *const pthread_mutex_t) -> c_int;
    pub fn pthread_cond_destroy(cond: *const pthread_cond_t) -> c_int;

    // == Thread local storage ==
    pub fn pthread_key_create(
        key: *mut pthread_key_t,
        routine: extern "C" fn(*mut c_void)) -> c_int;

    // pub fn pthread_key_delete(key: *mut pthread_key_t) -> c_int;

    pub fn pthread_getspecific(key: pthread_key_t) -> *mut c_void;

    pub fn pthread_setspecific(key: pthread_key_t, val: *const c_void) -> c_int;
}
