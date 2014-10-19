use std::{mem, ptr, uint};
use std::time::Duration;
use time::precise_time_ns;
use sync::Arc;
use locks::{MutexCell, CondVar};
use {Consume, Produce};

/// A queue in which values are contained by a linked list.
///
/// The current implementation is based on a mutex and two condition variables.
/// It is also mostly a placeholder until a lock-free version is implemented,
/// so it has not been tuned for performance.
pub struct LinkedQueue<T> {
    inner: Arc<QueueInner<T>>,
}

impl<T: Send> LinkedQueue<T> {
    pub fn new() -> LinkedQueue<T> {
        LinkedQueue::with_capacity(uint::MAX)
    }

    pub fn with_capacity(capacity: uint) -> LinkedQueue<T> {
        LinkedQueue {
            inner: Arc::new(QueueInner::new(capacity))
        }
    }
}

impl<T: Send> Consume<T> for LinkedQueue<T> {
    fn take_wait(&self, timeout: Duration) -> Option<T> {
        self.inner.take(Timeout::new(timeout))
    }
}

impl<T: Send> Produce<T> for LinkedQueue<T> {
    fn put_wait(&self, val: T, timeout: Duration) -> Result<(), T> {
        self.inner.put(val, Timeout::new(timeout))
    }
}

impl<T: Send> Clone for LinkedQueue<T> {
    fn clone(&self) -> LinkedQueue<T> {
        LinkedQueue { inner: self.inner.clone() }
    }
}

impl<T: Send> Collection for LinkedQueue<T> {
    fn len(&self) -> uint {
        self.inner.size()
    }
}

const NANOS_PER_MS: u64 = 1_000_000;

struct QueueInner<T> {
    core: MutexCell<Core<T>>,
    capacity: uint,
}

impl<T: Send> QueueInner<T> {
    fn new(capacity: uint) -> QueueInner<T> {
        QueueInner {
            core: MutexCell::new(Core::new()),
            capacity: capacity,
        }
    }

    fn take(&self, mut timeout: Timeout) -> Option<T> {
        let mut core = self.core.lock();

        loop {
            match core.pop_head() {
                Some(link) => {
                    // Notify any waiting producers
                    core.producer_notify();

                    // Return the value
                    return Some(link.val)
                }
                None => {
                    // If there is no more waiting, return None
                    if timeout.is_elapsed() {
                        return None;
                    }

                    // Wait for data
                    core.consumer_waiting += 1;
                    core.timed_wait(&core.consumer_condvar, timeout.remaining_ms());
                    core.consumer_waiting -= 1;

                    timeout.update();
                }
            }
        }
    }

    fn put(&self, mut val: T, mut timeout: Timeout) -> Result<(), T> {
        let mut core = self.core.lock();

        loop {
            match core.push_tail(val, self.capacity) {
                Ok(_) => {
                    core.consumer_notify();
                    return Ok(());
                }
                Err(v) => {
                    if timeout.is_elapsed() {
                        return Err(v);
                    }

                    val = v;

                    // Wait for availability
                    core.producer_waiting += 1;
                    core.timed_wait(&core.producer_condvar, timeout.remaining_ms());
                    core.consumer_waiting -= 1;

                    timeout.update();
                }
            }
        }
    }

    fn size(&self) -> uint {
        let core = self.core.lock();
        core.size
    }
}

struct Core<T> {
    size: uint,
    head: *mut Link<T>,
    tail: *mut Link<T>,
    // Synchronization
    consumer_waiting: uint,
    consumer_condvar: CondVar,
    producer_waiting: uint,
    producer_condvar: CondVar,
}

impl<T: Send> Core<T> {
    fn new() -> Core<T> {
        Core {
            size: 0,
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            // Synchronization
            consumer_waiting: 0,
            consumer_condvar: CondVar::new(),
            producer_waiting: 0,
            producer_condvar: CondVar::new(),
        }
    }

    fn pop_head(&mut self) -> Option<Box<Link<T>>> {
        if self.size == 0 {
            return None;
        }

        self.size -= 1;

        unsafe {
            let head: Box<Link<T>> = mem::transmute(self.head);

            // Update the head pointer
            self.head = head.next;

            if self.size == 0 {
                self.tail = ptr::null_mut();
            }

            if !self.head.is_null() {
                (*self.head).prev = ptr::null_mut();
            }

            Some(head)
        }
    }

    fn push_tail(&mut self, val: T, capacity: uint) -> Result<(), T> {
        if self.size >= capacity {
            return Err(val);
        }

        self.size += 1;

        unsafe {
            let tail: *mut Link<T> = mem::transmute(box Link::new(val, self.tail));

            if !self.tail.is_null() {
                (*self.tail).next = tail;
            }

            self.tail = tail;

            if self.size == 1 {
                self.head = tail;
            }
        }

        Ok(())
    }

    fn producer_notify(&self) {
        if self.producer_waiting > 0 {
            self.producer_condvar.signal();
        }
    }

    fn consumer_notify(&self) {
        if self.consumer_waiting > 0 {
            self.consumer_condvar.signal();
        }
    }
}

struct Link<T> {
    next: *mut Link<T>,
    prev: *mut Link<T>,
    val: T,
}

impl<T: Send> Link<T> {
    fn new(val: T, prev: *mut Link<T>) -> Link<T> {
        Link {
            next: ptr::null_mut(),
            prev: prev,
            val: val,
        }
    }
}

struct Timeout {
    remaining_ms: uint,
    time: u64,
}

impl Timeout {
    fn new(remaining: Duration) -> Timeout {
        Timeout {
            remaining_ms: remaining.num_milliseconds() as uint,
            time: precise_time_ns(),
        }
    }

    fn is_elapsed(&self) -> bool {
        self.remaining_ms == 0
    }

    fn remaining_ms(&self) -> uint {
        self.remaining_ms
    }

    fn update(&mut self) {
        let now = precise_time_ns();
        let elapsed = ((now - self.time) / NANOS_PER_MS) as uint;

        if elapsed > self.remaining_ms {
            self.remaining_ms = 0;
        } else {
            self.remaining_ms -= elapsed;
        }

        self.time = now;
    }
}

#[cfg(test)]
mod test {
    use std::io::timer::sleep;
    use std::time::Duration;
    use {LinkedQueue, Consume, Produce};

    #[test]
    pub fn test_single_threaded_put_take() {
        let q = LinkedQueue::new();

        // Check the length
        assert_eq!(0, q.len());

        // Put a value
        assert!(q.put(1u).is_ok());

        // Check the length again
        assert_eq!(1, q.len());

        // Remove the value
        assert_eq!(1u, q.take().unwrap());

        // Check the length
        assert_eq!(0, q.len());

        // Try taking on an empty queue
        assert!(q.take().is_none());
    }

    #[test]
    pub fn test_single_consumer_single_producer() {
        let c = LinkedQueue::new();
        let p = c.clone();

        spawn(proc() {
            sleep(millis(10));

            for i in range(0, 10_000u) {
                p.put(i).unwrap();
            }
        });

        let mut i = 0;

        while let Some(v) = c.take_wait(millis(50)) {
            assert_eq!(i, v);
            i += 1;
        }

        assert_eq!(10_000, i);
    }

    #[test]
    pub fn test_single_consumer_multi_producer() {
        let c = LinkedQueue::new();

        for t in range(0, 10u) {
            let p = c.clone();

            spawn(proc() {
                sleep(millis(10));

                for i in range(0, 10_000u) {
                    p.put((t, i)).unwrap();
                }
            });
        }

        let mut vals = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

        while let Some((t, v)) = c.take_wait(millis(50)) {
            assert_eq!(vals[t], v);
            vals[t] += 1;
        }

        for &v in vals.iter() {
            assert_eq!(10_000, v);
        }
    }

    #[test]
    pub fn test_multi_consumer_multi_producer() {
        let queue = LinkedQueue::new();
        let results = LinkedQueue::new();

        // Producers
        for t in range(0, 10u) {
            let producer = queue.clone();

            spawn(proc() {
                sleep(Duration::milliseconds(10));

                for i in range(1, 10_000u) {
                    producer.put((t, i)).unwrap();
                }
            });
        }

        // Consumers
        for _ in range(0, 10u) {
            let consumer = queue.clone();
            let results = results.clone();

            spawn(proc() {
                let mut vals = vec![];
                let mut per_producer = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

                while let Some((t, v)) = consumer.take_wait(millis(50)) {
                    assert!(per_producer[t] < v);
                    per_producer[t] = v;

                    vals.push((t, v));
                }

                results.put(vals).unwrap();
            });
        }

        let mut all_vals = vec![];

        for _ in range(0, 10u) {
            match results.take_wait(millis(5_000)) {
                Some(vals) => {
                    // Probably too strict
                    assert!(vals.len() >= 9_000 && vals.len() <= 11_000, "uneven batch size {}", vals.len());
                    for &v in vals.iter() {
                        all_vals.push(v);
                    }
                }
                None => fail!("only receives `{}` result batches", all_vals.len()),
            }
        }

        all_vals.sort();

        let mut per_producer = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1];

        for &(t, v) in all_vals.iter() {
            assert_eq!(per_producer[t], v);
            per_producer[t] += 1;
        }

        for &v in per_producer.iter() {
            assert_eq!(10_000, v);
        }
    }

    #[test]
    pub fn test_queue_with_capacity() {
        let queue = LinkedQueue::with_capacity(8);

        for i in range(0, 8u) {
            assert!(queue.put(i).is_ok());
        }

        assert_eq!(Err(8), queue.put(8));
        assert_eq!(Some(0), queue.take());

        assert!(queue.put(8).is_ok());

        for i in range(1, 9u) {
            assert_eq!(Some(i), queue.take());
        }
    }

    #[test]
    pub fn test_multi_producer_at_capacity() {
        let queue = LinkedQueue::with_capacity(8);

        for _ in range(0, 8u) {
            let queue = queue.clone();

            spawn(proc() {
                for i in range(0, 30_000u) {
                    assert!(queue.put_wait(i, millis(1000)).is_ok());
                }
            });
        }

        let mut n = 0;

        while n < 8 * 30_000u {
            assert!(queue.take_wait(millis(5000)).is_some());
            n += 1;
        }

        assert_eq!(8 * 30_000u, n);
    }

    fn millis(num: uint) -> Duration {
        Duration::milliseconds(num as i64)
    }
}
