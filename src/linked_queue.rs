use super::{Queue, SyncQueue};
use time;
use std::{mem, ptr, ops, usize, u32};
use std::sync::{Arc, Mutex, MutexGuard, Condvar};
use std::sync::atomic::{self, AtomicUsize, Ordering};

/// A queue in which values are contained by a linked list.
///
/// The current implementation is based on a mutex and two condition variables.
/// It is also mostly a placeholder until a lock-free version is implemented,
/// so it has not been tuned for performance.
pub struct LinkedQueue<T: Send> {
    inner: Arc<QueueInner<T>>,
}

impl<T: Send> LinkedQueue<T> {
    /// Constructs a new, empty `LinkedQueue<T>` with capacity `usize::MAX`.
    pub fn new() -> LinkedQueue<T> {
        LinkedQueue::with_capacity(usize::MAX)
    }

    /// Constructs a new, empty `LinkedQueue<T>` with the specified capacity.
    pub fn with_capacity(capacity: usize) -> LinkedQueue<T> {
        LinkedQueue {
            inner: Arc::new(QueueInner::new(capacity))
        }
    }

    /// Returns the number of elements in the queue.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the queue contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds the element `e` to the queue if possible.
    ///
    /// # Errors
    ///
    /// A call to `offer` will fail if the queue is full; the provided element
    /// `e` is returned in the `Err` variant.
    pub fn offer(&self, e: T) -> Result<(), T> {
        self.inner.offer(e)
    }

    /// Adds the element `e` to the queue, blocking for up to `ms` milliseconds.
    ///
    /// # Errors
    ///
    /// A call to `offer_ms` will fail if the element cannot be added within the
    /// timeout; the provided element `e` is returned in the `Err` variant.
    pub fn offer_ms(&self, e: T, ms: u32) -> Result<(), T> {
        self.inner.offer_ms(e, ms)
    }

    /// Adds the element `e` to the queue, blocking until it can be added.
    // TODO(soon): decide on documenting panic?
    pub fn put(&self, e: T) {
        self.inner.put(e);
    }

    /// Takes from the queue if there is an element available.
    pub fn poll(&self) -> Option<T> {
        self.inner.poll()
    }

    /// Takes from the queue, blocking for up to `ms` milliseconds.
    pub fn poll_ms(&self, ms: u32) -> Option<T> {
        self.inner.poll_ms(ms)
    }

    /// Takes from the queue, blocking until there is an element available.
    // TODO(soon): decide on documenting panic?
    pub fn take(&self) -> T {
        self.inner.take()
    }
}

impl<T: Send> Queue<T> for LinkedQueue<T> {
    fn poll(&self) -> Option<T> {
        LinkedQueue::poll(self)
    }

    fn is_empty(&self) -> bool {
        LinkedQueue::is_empty(self)
    }

    fn offer(&self, e: T) -> Result<(), T> {
        LinkedQueue::offer(self, e)
    }
}

impl<T: Send> SyncQueue<T> for LinkedQueue<T> {
    fn take(&self) -> T {
        LinkedQueue::take(self)
    }

    fn put(&self, e: T) {
        LinkedQueue::put(self, e)
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
struct QueueInner<T: Send> {

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
        self.offer_ms(e, u32::MAX)
            .ok().expect("something went wrong");
    }

    fn offer(&self, e: T) -> Result<(), T> {
        if self.len() == self.capacity {
            return Err(e);
        }

        self.offer_ms(e, 0)
    }

    fn offer_ms(&self, e: T, mut dur: u32) -> Result<(), T> {
        // Acquire the write lock
        let mut last = self.last.lock()
            .ok().expect("something went wrong");

        if self.len() == self.capacity {
            let mut now = time::precise_time_ns();

            loop {
                if dur == 0 {
                    return Err(e);
                }

                last = self.not_full.wait_timeout_ms(last, dur)
                    .ok().expect("something went wrong").0;

                if self.len() != self.capacity {
                    break;
                }

                let n = time::precise_time_ns();
                let d = (n - now) / 1_000_000;

                if d >= dur as u64 {
                    dur = 0;
                } else {
                    dur -= d as u32;
                    now = n;
                }
            }
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
        self.poll_ms(u32::MAX)
            .expect("something went wrong")
    }

    fn poll(&self) -> Option<T> {
        if self.len() == 0 {
            // Fast path check
            return None;
        }

        self.poll_ms(0)
    }

    fn poll_ms(&self, mut dur: u32) -> Option<T> {
        // Acquire the read lock
        let mut head = self.head.lock()
            .ok().expect("something went wrong");

        if self.len() == 0 {
            let mut now = time::precise_time_ns();

            loop {
                if dur == 0 {
                    return None;
                }

                head = self.not_empty.wait_timeout_ms(head, dur)
                    .ok().expect("something went wrong").0;

                if self.len() != 0 {
                    break;
                }

                let n = time::precise_time_ns();
                let d = (n - now) / 1_000_000;

                if d >= dur as u64 {
                    dur = 0;
                } else {
                    dur -= d as u32;
                    now = n;
                }
            }
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

struct NodePtr<T: Send> {
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

impl<T: Send> Clone for NodePtr<T> {
    fn clone(&self) -> NodePtr<T> {
        NodePtr { ptr: self.ptr }
    }
}

impl<T: Send> Copy for NodePtr<T> {}
unsafe impl<T: Send> Send for NodePtr<T> {}
