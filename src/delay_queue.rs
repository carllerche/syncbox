use super::{Queue, SyncQueue};
use std::collections::BinaryHeap;
use std::cmp::{self, PartialOrd, Ord, PartialEq, Eq, Ordering};
use std::ops;
use std::sync::{Arc, Mutex, MutexGuard, Condvar};
use time::{Duration, SteadyTime};

/// A value that should not be used until the delay has expired.
pub trait Delayed {
    fn delay(&self) -> Duration;
}

impl<T: Delayed> Delayed for Option<T> {
    /// The delay associated with the value
    fn delay(&self) -> Duration {
        match *self {
            Some(ref v) => v.delay(),
            None => Duration::nanoseconds(0),
        }
    }
}

/// Associate a delay with a value
#[derive(Eq, PartialEq, Ord, PartialOrd, Debug)]
pub struct Delay<T>(pub T, pub Duration);

impl<T> Delay<T> {
    pub fn unwrap(self) -> T {
        self.0
    }
}

impl<T> Delayed for Delay<T> {
    fn delay(&self) -> Duration {
        self.1
    }
}

impl<T> ops::Deref for Delay<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> ops::DerefMut for Delay<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// An unbounded blocking queue of delayed values. When a value is pushed onto
/// the queue, a delay is included. The value will only be able to be popped
/// off once the specified delay has expired. The head of the queue is the
/// value whose delay is expired and furthest in the past.
pub struct DelayQueue<T: Delayed + Send> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    queue: Mutex<BinaryHeap<Entry<T>>>,
    condvar: Condvar,
}

impl<T: Delayed + Send> DelayQueue<T> {
    /// Create a new `DelayQueue`
    pub fn new() -> DelayQueue<T> {
        DelayQueue {
            inner: Arc::new(Inner {
                queue: Mutex::new(BinaryHeap::new()),
                condvar: Condvar::new(),
            })
        }
    }

    /// Retrieves and removes the head of the queue, blocking if necessary for
    /// up to `timeout`.
    pub fn poll_timeout(&self, timeout: Duration) -> Option<T> {
        let end = SteadyTime::now() + timeout;
        let mut queue = self.inner.queue.lock().unwrap();

        loop {
            let now = SteadyTime::now();

            if now >= end {
                return None;
            }

            let wait_until = match queue.peek() {
                Some(e) if e.time <= now => break,
                Some(e) => cmp::min(end, e.time),
                None => end,
            };

            // TODO: Check the cast
            let timeout = (wait_until - now).num_milliseconds() as u32;

            queue = self.inner.condvar.wait_timeout_ms(queue, timeout).unwrap().0;
        }

        Some(self.finish_pop(queue))
    }

    fn finish_pop<'a>(&self, mut queue: MutexGuard<'a, BinaryHeap<Entry<T>>>) -> T {
        if queue.len() > 1 {
            self.inner.condvar.notify_all();
        }

        queue.pop().unwrap().val
    }
}

impl<T: Delayed + Send> Queue<T> for DelayQueue<T> {
    fn poll(&self) -> Option<T> {
        let queue = self.inner.queue.lock().unwrap();

        match queue.peek() {
            Some(e) if e.time > SteadyTime::now() => return None,
            Some(_) => {}
            None => return None,
        }

        Some(self.finish_pop(queue))
    }

    fn is_empty(&self) -> bool {
        let queue = self.inner.queue.lock().unwrap();
        queue.is_empty()
    }

    fn offer(&self, e: T) -> Result<(), T> {
        let delay = e.delay();
        let entry = Entry::new(e, delay);
        let mut queue = self.inner.queue.lock().unwrap();

        match queue.peek() {
            Some(e) => {
                if entry.time < e.time {
                    self.inner.condvar.notify_all()
                }
            }
            None => self.inner.condvar.notify_all(),
        }

        queue.push(entry);
        Ok(())
    }
}

impl<T: Delayed + Send> SyncQueue<T> for DelayQueue<T> {
    fn take(&self) -> T {
        enum Need {
            Wait,
            WaitTimeout(Duration),
        }

        let mut queue = self.inner.queue.lock().unwrap();

        loop {
            let now = SteadyTime::now();

            let need = match queue.peek() {
                Some(e) => {
                    if e.time <= now {
                        break;
                    }

                    Need::WaitTimeout(e.time - now)
                }
                None => Need::Wait
            };

            queue = match need {
                Need::Wait => {
                    self.inner.condvar.wait(queue).unwrap()
                }
                Need::WaitTimeout(t) => {
                    // TODO: Check the cast
                    let timeout = t.num_milliseconds() as u32;

                    self.inner.condvar.wait_timeout_ms(queue, timeout).unwrap().0
                }
            };
        }

        self.finish_pop(queue)
    }

    fn put(&self, e: T) {
        self.offer(e).ok().unwrap();
    }
}

impl<T: Delayed + Send> Clone for DelayQueue<T> {
    fn clone(&self) -> DelayQueue<T> {
        DelayQueue { inner: self.inner.clone() }
    }
}

struct Entry<T> {
    val: T,
    time: SteadyTime,
}

impl<T> Entry<T> {
    fn new(val: T, delay: Duration) -> Entry<T> {
        Entry {
            val: val,
            time: SteadyTime::now() + delay,
        }
    }
}

impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Entry<T>) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Entry<T>) -> Ordering {
        // BinaryHeap is a max heap, so reverse
        self.time.cmp(&other.time).reverse()
    }
}

impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Entry<T>) -> bool {
        self.time == other.time
    }
}

impl<T> Eq for Entry<T> {}
