use super::Consume;
use std::{mem, ptr, ops, usize};
use std::sync::{Arc, Mutex, MutexGuard, Condvar};
use std::sync::atomic::{self, AtomicUsize, Ordering};
use std::time::Duration;

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
        LinkedQueue::with_capacity(usize::MAX)
    }

    pub fn with_capacity(capacity: usize) -> LinkedQueue<T> {
        LinkedQueue {
            inner: Arc::new(QueueInner::new(capacity))
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn offer(&self, e: T) -> Result<(), T> {
        self.inner.offer(e)
    }

    pub fn put(&self, e: T) {
        self.inner.put(e);
    }

    pub fn poll(&self) -> Option<T> {
        self.inner.poll()
    }

    /// Takes from the queue, blocking until there is an element available.
    pub fn take(&self) -> T {
        self.inner.take()
    }
}

impl<T: Send> Consume<T> for LinkedQueue<T> {
    fn poll(&self) -> Option<T> {
        LinkedQueue::poll(self)
    }

    fn take(&self) -> T {
        LinkedQueue::take(self)
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

    // Maximum number of elements the queue can contain at one time
    capacity: usize,

    // Current number of elements
    count: AtomicUsize,

    // Lock held by take, poll, etc
    head: Mutex<NodePtr<T>>,

    // Lock held by put, offer, etc
    last: Mutex<NodePtr<T>>,

    // Wait queue for waiting takes
    not_empty: Condvar,

    // Wait queue for waiting puts
    not_full: Condvar,
}

impl<T: Send> QueueInner<T> {
    fn new(capacity: usize) -> QueueInner<T> {
        let head = NodePtr::new(Node::empty());

        QueueInner {
            capacity: capacity,
            count: AtomicUsize::new(0),
            head: Mutex::new(head),
            last: Mutex::new(head),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
        }
    }

    fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    fn put(&self, e: T) {
        self.offer_for(e, Duration::max_value())
            .ok().expect("something went wrong");
    }

    fn offer(&self, e: T) -> Result<(), T> {
        if self.len() == self.capacity {
            return Err(e);
        }

        self.offer_for(e, Duration::milliseconds(0))
    }

    fn offer_for(&self, e: T, dur: Duration) -> Result<(), T> {
        // Acquire the write lock
        let mut last = self.last.lock()
            .ok().expect("something went wrong");

        while self.len() == self.capacity {
            if dur.num_milliseconds() <= 0 {
                return Err(e);
            }

            last = self.not_full.wait(last)
                .ok().expect("something went wrong");
        }

        // Enqueue the node
        enqueue(Node::new(e), &mut last);

        // Increment the count
        let cnt = self.count.fetch_add(1, Ordering::Release);

        if cnt + 1 < self.capacity {
            self.not_full.notify_one();
        }

        drop(last);

        self.notify_not_empty();

        Ok(())
    }

    fn take(&self) -> T {
        self.poll_for(Duration::max_value())
            .expect("something went wrong")
    }

    fn poll(&self) -> Option<T> {
        if self.len() == 0 {
            // Fast path check
            return None;
        }

        self.poll_for(Duration::milliseconds(0))
    }

    fn poll_for(&self, dur: Duration) -> Option<T> {
        // Acquire the read lock
        let mut head = self.head.lock()
            .ok().expect("something went wrong");

        while self.len() == 0 {
            if dur.num_milliseconds() <= 0 {
                return None;
            }

            head = self.not_empty.wait(head)
                .ok().expect("something went wrong");
        }

        // Acquire memory from write side
        atomic::fence(Ordering::Acquire);

        // At this point, we are guaranteed to be able to dequeue a value
        let val = dequeue(&mut head);
        let cnt = self.count.fetch_sub(1, Ordering::Relaxed);

        if cnt > 1 {
            self.not_empty.notify_one();
        }

        // Release the lock here so that acquire the write lock does not result
        // in a deadlock
        drop(head);

        if cnt == self.capacity {
            self.notify_not_full();
        }

        Some(val)
    }

    // Signals a waiting put. Called only from take / poll
    fn notify_not_full(&self) {
        let _l = self.last.lock()
            .ok().expect("something went wrong");

        self.not_full.notify_one();
    }

    fn notify_not_empty(&self) {
        let _l = self.head.lock()
            .ok().expect("something went wrong");

        self.not_empty.notify_one();
    }
}

#[unsafe_destructor]
impl<T: Send> Drop for QueueInner<T> {
    fn drop(&mut self) {
        while let Some(_) = self.poll() {
        }
    }
}

fn dequeue<T: Send>(mut head: &mut MutexGuard<NodePtr<T>>) -> T {
    let h = **head;
    let mut first = h.next;
    **head = first;
    h.free();
    first.item.take().expect("item already consumed")
}

fn enqueue<T: Send>(node: Node<T>, mut last: &mut MutexGuard<NodePtr<T>>) {
    let ptr = NodePtr::new(node);

    last.next = ptr;
    **last = ptr;
}

struct Node<T: Send> {
    next: NodePtr<T>,
    item: Option<T>,
}

impl<T: Send> Node<T> {
    fn new(val: T) -> Node<T> {
        Node {
            next: NodePtr::null(),
            item: Some(val),
        }
    }

    fn empty() -> Node<T> {
        Node {
            next: NodePtr::null(),
            item: None,
        }
    }
}

struct NodePtr<T> {
    ptr: *mut Node<T>,
}

impl<T: Send> NodePtr<T> {
    fn new(node: Node<T>) -> NodePtr<T> {
        NodePtr { ptr: unsafe { mem::transmute(Box::new(node)) }}
    }

    fn null() -> NodePtr<T> {
        NodePtr { ptr: ptr::null_mut() }
    }

    fn free(self) {
        let NodePtr { ptr } = self;
        let _: Box<Node<T>> = unsafe { mem::transmute(ptr) };
    }
}

impl<T: Send> ops::Deref for NodePtr<T> {
    type Target = Node<T>;

    fn deref(&self) -> &Node<T> {
        unsafe { mem::transmute(self.ptr) }
    }
}

impl<T: Send> ops::DerefMut for NodePtr<T> {
    fn deref_mut(&mut self) -> &mut Node<T> {
        unsafe { mem::transmute(self.ptr) }
    }
}

impl<T: Send> Copy for NodePtr<T> {}
unsafe impl<T: Send> Send for NodePtr<T> {}

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
        assert!(q.poll().is_none());
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

        assert!(c.poll().is_none());
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
        for t in range(0, 5u) {
            let producer = queue.clone();

            Thread::spawn(move || {
                sleep(Duration::milliseconds(10));

                for i in range(1, 1_000u) {
                    producer.put((t, i));
                    Thread::yield_now();
                }

                // Put an extra val to signal consumers to exit
                producer.put((t, 1_000_000));
            });
        }

        // Consumers
        for _ in range(0, 5u) {
            let consumer = queue.clone();
            let results = results.clone();

            Thread::spawn(move || {
                let mut vals = vec![];
                let mut per_producer = [0, 0, 0, 0, 0];

                loop {
                    let (t, v) = consumer.take();

                    if v > 1_000u {
                        break;
                    }

                    assert!(per_producer[t] < v);
                    per_producer[t] = v;

                    vals.push((t, v));
                    Thread::yield_now();
                }

                results.put(vals);
            });
        }

        let mut all_vals = vec![];

        for _ in range(0, 5u) {
            let vals = results.take();

            // TODO: Figure out this assertion
            // assert!(vals.len() >= 50 && vals.len() <= 3_000, "uneven batch size {}", vals.len());

            for &v in vals.iter() {
                all_vals.push(v);
            }
        }

        all_vals.sort();

        let mut per_producer = [1, 1, 1, 1, 1];

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
            assert!(queue.offer(i).is_ok());
        }

        assert_eq!(Err(8), queue.offer(8));
        assert_eq!(Some(0), queue.poll());

        assert!(queue.offer(8).is_ok());

        for i in range(1, 9u) {
            assert_eq!(Some(i), queue.poll());
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
