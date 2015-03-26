#![feature(std_misc)]

extern crate syncbox;

#[macro_use]
extern crate log;
extern crate time;
extern crate env_logger;

mod util;

fn spawn<F: FnOnce() + Send + 'static>(f: F) {
    use std::thread;
    thread::spawn(f);
}

fn sleep_ms(ms: usize) {
    use std::thread;
    use std::time::Duration;
    use time::precise_time_ns;

    let start = precise_time_ns();
    let target = start + (ms as u64) * 1_000_000;

    loop {
        let now = precise_time_ns();

        if now > target {
            return;
        }

        thread::park_timeout(Duration::nanoseconds((target - now) as i64));
    }
}
