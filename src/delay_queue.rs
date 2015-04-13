use super::{Queue, SyncQueue};
use std::collections::BinaryHeap;
use std::cmp::{self, PartialOrd, Ord, PartialEq, Eq, Ordering};
use std::sync::{Mutex, MutexGuard, Condvar};
use time::{Duration, SteadyTime};

/// An unbounded blocking queue of delayed values. When a value is pushed onto
/// the queue, a delay is included. The value will only be able to be popped
/// off once the specified delay has expired. The head of the queue is the
/// value whose delay is expired and furthest in the past.
pub struct DelayQueue<T: Send> {
    queue: Mutex<BinaryHeap<Entry<T>>>,
    condvar: Condvar,
}

impl<T: Send> DelayQueue<T> {
    /// Create a new `DelayQueue`
    pub fn new() -> DelayQueue<T> {
        DelayQueue {
            queue: Mutex::new(BinaryHeap::new()),
            condvar: Condvar::new(),
        }
    }

    /// Push a value on the queue that will be available after `delay` has
    /// expired.
    pub fn offer_delay(&self, val: T, delay: Duration) {
        let entry = Entry::new(val, delay);
        let mut queue = self.queue.lock().unwrap();

        match queue.peek() {
            Some(e) => {
                if entry.time < e.time {
                    self.condvar.notify_all()
                }
            }
            None => self.condvar.notify_all(),
        }

        queue.push(entry);
    }

    /// Retrieves and removes the head of the queue, blocking if necessary for
    /// up to `timeout`.
    pub fn poll_timeout(&self, timeout: Duration) -> Option<T> {
        let end = SteadyTime::now() + timeout;
        let mut queue = self.queue.lock().unwrap();

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

            queue = self.condvar.wait_timeout_ms(queue, timeout).unwrap().0;
        }

        Some(self.finish_pop(queue))
    }

    fn finish_pop<'a>(&self, mut queue: MutexGuard<'a, BinaryHeap<Entry<T>>>) -> T {
        if queue.len() > 1 {
            self.condvar.notify_all();
        }

        queue.pop().unwrap().val
    }
}

impl<T: Send> Queue<T> for DelayQueue<T> {
    fn poll(&self) -> Option<T> {
        let queue = self.queue.lock().unwrap();

        match queue.peek() {
            Some(e) if e.time > SteadyTime::now() => return None,
            Some(_) => {}
            None => return None,
        }

        Some(self.finish_pop(queue))
    }

    fn is_empty(&self) -> bool {
        let queue = self.queue.lock().unwrap();
        queue.is_empty()
    }

    fn offer(&self, e: T) -> Result<(), T> {
        self.offer_delay(e, Duration::nanoseconds(0));
        Ok(())
    }
}

impl<T: Send> SyncQueue<T> for DelayQueue<T> {
    fn take(&self) -> T {
        enum Need {
            Wait,
            WaitTimeout(Duration),
        }

        let mut queue = self.queue.lock().unwrap();

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
                    self.condvar.wait(queue).unwrap()
                }
                Need::WaitTimeout(t) => {
                    // TODO: Check the cast
                    let timeout = t.num_milliseconds() as u32;

                    self.condvar.wait_timeout_ms(queue, timeout).unwrap().0
                }
            };
        }

        self.finish_pop(queue)
    }

    fn put(&self, e: T) {
        self.offer(e).ok().unwrap();
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
