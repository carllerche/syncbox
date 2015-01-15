use std::{mem, ptr, uint};
use std::sync::{Arc, Mutex, Condvar};

// == WARNING: CURRENTLY BUGGY ==

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

    pub fn len(&self) -> uint {
        self.inner.size()
    }

    /// Takes from the queue, blocking until there is an element available.
    pub fn take(&self) -> T {
        self.inner.take(true).unwrap()
    }

    pub fn take_opt(&self) -> Option<T> {
        self.inner.take(false)
    }

    /// Put an element into the queue, blocking until there is capacity.
    pub fn put(&self, val: T) {
        let _ = self.inner.put(val, true);
    }

    pub fn put_opt(&self, val: T) -> Result<(), T> {
        self.inner.put(val, false)
    }
}

impl<T: Send> Clone for LinkedQueue<T> {
    fn clone(&self) -> LinkedQueue<T> {
        LinkedQueue { inner: self.inner.clone() }
    }
}

//  A variant of the "two lock queue" algorithm.  The putLock gates
//  entry to put (and offer), and has an associated condition for
//  waiting puts.  Similarly for the takeLock.  The "count" field
//  that they both rely on is maintained as an atomic to avoid
//  needing to get both locks in most cases. Also, to minimize need
//  for puts to get takeLock and vice-versa, cascading notifies are
//  used. When a put notices that it has enabled at least one take,
//  it signals taker. That taker in turn signals others if more
//  items have been entered since the signal. And symmetrically for
//  takes signalling puts. Operations such as remove(Object) and
//  iterators acquire both locks.
//
//  Visibility between writers and readers is provided as follows:
//
//  Whenever an element is enqueued, the putLock is acquired and
//  count updated.  A subsequent reader guarantees visibility to the
//  enqueued Node by either acquiring the putLock (via fullyLock)
//  or by acquiring the takeLock, and then reading n = count.get();
//  this gives visibility to the first n items.
//
//  To implement weakly consistent iterators, it appears we need to
//  keep all Nodes GC-reachable from a predecessor dequeued Node.
//  That would cause two problems:
//  - allow a rogue Iterator to cause unbounded memory retention
//  - cause cross-generational linking of old Nodes to new Nodes if
//    a Node was tenured while live, which generational GCs have a
//    hard time dealing with, causing repeated major collections.
//  However, only non-deleted Nodes need to be reachable from
//  dequeued Nodes, and reachability does not necessarily have to
//  be of the kind understood by the GC.  We use the trick of
//  linking a Node that has just been dequeued to itself.  Such a
//  self-link implicitly means to advance to head.next.
struct QueueInner<T> {
    core: Mutex<Core<T>>,
    consumer_condvar: Condvar,
    producer_condvar: Condvar,
    capacity: uint,
}

impl<T: Send> QueueInner<T> {
    fn new(capacity: uint) -> QueueInner<T> {
        QueueInner {
            core: Mutex::new(Core::new()),
            consumer_condvar: Condvar::new(),
            producer_condvar: Condvar::new(),
            capacity: capacity,
        }
    }

    fn take(&self, block: bool) -> Option<T> {
        let mut core = self.core.lock().unwrap();

        loop {
            match core.pop_head() {
                Some(link) => {
                    // Notify any waiting producers
                    self.producer_notify(&*core);

                    // Return the value
                    return Some(link.val)
                }
                None => {
                    if !block { return None; }

                    // Wait for data
                    core.consumer_waiting += 1;
                    // TODO: Respect timeout once Rust exposes timeouts
                    core = self.consumer_condvar.wait(core).unwrap();
                    core.consumer_waiting -= 1;
                }
            }
        }
    }

    fn put(&self, mut val: T, block: bool) -> Result<(), T> {
        let mut core = self.core.lock().unwrap();

        loop {
            match core.push_tail(val, self.capacity) {
                Ok(_) => {
                    self.consumer_notify(&*core);
                    return Ok(());
                }
                Err(v) => {
                    if !block { return Err(v) };

                    val = v;

                    // Wait for availability
                    core.producer_waiting += 1;
                    // TODO: Respect timeout once Rust exposes timeouts
                    core = self.producer_condvar.wait(core).unwrap();
                    core.consumer_waiting -= 1;
                }
            }
        }
    }

    fn producer_notify(&self, core: &Core<T>) {
        if core.producer_waiting > 0 {
            self.producer_condvar.notify_one();
        }
    }

    fn consumer_notify(&self, core: &Core<T>) {
        if core.consumer_waiting > 0 {
            self.consumer_condvar.notify_one();
        }
    }

    fn size(&self) -> uint {
        let core = self.core.lock().unwrap();
        core.size
    }
}

struct Core<T> {
    size: uint,
    head: *mut Link<T>,
    tail: *mut Link<T>,
    // Synchronization
    consumer_waiting: uint,
    producer_waiting: uint,
}

impl<T: Send> Core<T> {
    fn new() -> Core<T> {
        Core {
            size: 0,
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            // Synchronization
            consumer_waiting: 0,
            producer_waiting: 0,
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
            let tail: *mut Link<T> = mem::transmute(Box::new(Link::new(val, self.tail)));

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

unsafe impl<T: Send> Send for Core<T> {
}

#[cfg(test)]
mod test {
    use super::LinkedQueue;
    use std::io::timer::sleep;
    use std::time::Duration;
    use std::thread::Thread;

    #[test]
    pub fn test_single_threaded_put_take() {
        let q = LinkedQueue::new();

        // Check the length
        assert_eq!(0, q.len());

        // Put a value
        q.put(1u);

        // Check the length again
        assert_eq!(1, q.len());

        // Remove the value
        assert_eq!(1u, q.take());

        // Check the length
        assert_eq!(0, q.len());

        // Try taking on an empty queue
        assert!(q.take_opt().is_none());
    }

    #[test]
    pub fn test_single_consumer_single_producer() {
        let c = LinkedQueue::new();
        let p = c.clone();

        Thread::spawn(move || {
            sleep(millis(10));

            for i in range(0, 10_000u) {
                p.put(i);
            }
        });

        for i in range(0, 10_000) {
            assert_eq!(i, c.take());
        }

        assert!(c.take_opt().is_none());
    }

    #[test]
    pub fn test_single_consumer_multi_producer() {
        let c = LinkedQueue::new();

        for t in range(0, 10u) {
            let p = c.clone();

            Thread::spawn(move || {
                sleep(millis(10));

                for i in range(0, 10_000u) {
                    p.put((t, i));
                }
            });
        }

        let mut vals = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

        for _ in range(0, 10 * 10_000u) {
            let (t, v) = c.take();
            assert_eq!(vals[t], v);
            vals[t] += 1;
        }
    }

    #[test]
    pub fn test_multi_consumer_multi_producer() {
        let queue = LinkedQueue::new();
        let results = LinkedQueue::new();

        // Producers
        for t in range(0, 10u) {
            let producer = queue.clone();

            Thread::spawn(move || {
                sleep(Duration::milliseconds(10));

                for i in range(1, 1_000u) {
                    producer.put((t, i));
                }

                // Put an extra val to signal consumers to exit
                producer.put((t, 1_000_000));
            });
        }

        // Consumers
        for _ in range(0, 10u) {
            let consumer = queue.clone();
            let results = results.clone();

            Thread::spawn(move || {
                let mut vals = vec![];
                let mut per_producer = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

                loop {
                    let (t, v) = consumer.take();

                    if v > 1_000u {
                        break;
                    }

                    assert!(per_producer[t] < v);
                    per_producer[t] = v;

                    vals.push((t, v));
                }

                results.put(vals);
            });
        }

        let mut all_vals = vec![];

        for _ in range(0, 10u) {
            let vals = results.take();

            // Probably too strict
            assert!(vals.len() >= 900 && vals.len() <= 1_100, "uneven batch size {}", vals.len());
            for &v in vals.iter() {
                all_vals.push(v);
            }
        }

        all_vals.sort();

        let mut per_producer = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1];

        for &(t, v) in all_vals.iter() {
            assert_eq!(per_producer[t], v);
            per_producer[t] += 1;
        }

        for &v in per_producer.iter() {
            assert_eq!(1_000, v);
        }
    }

    #[test]
    pub fn test_queue_with_capacity() {
        let queue = LinkedQueue::with_capacity(8);

        for i in range(0, 8u) {
            assert!(queue.put_opt(i).is_ok());
        }

        assert_eq!(Err(8), queue.put_opt(8));
        assert_eq!(Some(0), queue.take_opt());

        assert!(queue.put_opt(8).is_ok());

        for i in range(1, 9u) {
            assert_eq!(Some(i), queue.take_opt());
        }
    }

    #[test]
    pub fn test_multi_producer_at_capacity() {
        let queue = LinkedQueue::with_capacity(8);

        for _ in range(0, 8u) {
            let queue = queue.clone();

            Thread::spawn(move || {
                for i in range(0, 1_000u) {
                    queue.put(i);
                }
            });
        }

        for _ in range(0, 8 * 1_000u) {
            queue.take();
        }
    }

    fn millis(num: uint) -> Duration {
        Duration::milliseconds(num as i64)
    }
}
