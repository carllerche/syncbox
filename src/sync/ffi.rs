use libc::c_int;
pub use libc::timespec;
pub use libc::consts::os::posix88::{EBUSY, ETIMEDOUT};
pub use self::os::arch::{
    pthread_mutex_t,
    pthread_cond_t,
    PTHREAD_MUTEX_INITIALIZER,
    PTHREAD_COND_INITIALIZER,
};

#[cfg(target_os = "macos")]
mod os {
    #[cfg(target_arch = "x86_64")]
    pub mod arch {

        #[repr(C)]
        pub struct pthread_mutex_t {
            _sig: u64,
            _opaque: [u64, ..7],
        }

        pub static PTHREAD_MUTEX_INITIALIZER: pthread_mutex_t =
            pthread_mutex_t {
                _sig: 0x32AAABA7,
                _opaque: [0, 0, 0, 0, 0, 0, 0],
            };

        #[repr(C)]
        pub struct pthread_cond_t {
            _sig: u64,
            _opaque: [u64, ..5]
        }

        pub static PTHREAD_COND_INITIALIZER: pthread_cond_t =
            pthread_cond_t {
                _sig: 0x3CB0B1BB,
                _opaque: [0, 0, 0, 0, 0],
            };
    }
}

#[link(name = "pthread")]
extern {

    // == Mutex
    pub fn pthread_mutex_lock(mutex: *const pthread_mutex_t) -> c_int;
    pub fn pthread_mutex_trylock(mutex: *const pthread_mutex_t) -> c_int;
    pub fn pthread_mutex_unlock(mutex: *const pthread_mutex_t) -> c_int;

    // == CondVar
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
}
