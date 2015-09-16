use {LinkedQueue, Queue, SyncQueue, Run, Task, Delayed, DelayQueue};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{thread, usize};
use time::Duration;

/// A queue that can be used to back a thread pool
pub trait WorkQueue<T: Send> : SyncQueue<Option<T>> + Clone + Send + 'static {
}

impl<T: Send, Q: SyncQueue<Option<T>> + Clone + Send + 'static> WorkQueue<T> for Q {
}

pub struct ThreadPool<T: Task+'static, Q: WorkQueue<T> = LinkedQueue<Option<T>>> {
    inner: Arc<ThreadPoolInner<T, Q>>,
}

impl<T: Task+'static> ThreadPool<T, LinkedQueue<Option<T>>> {
    pub fn fixed_size(size: u32) -> ThreadPool<T, LinkedQueue<Option<T>>> {
        ThreadPool::new(size, size, LinkedQueue::with_capacity(usize::MAX))
    }

    pub fn single_thread() -> ThreadPool<T, LinkedQueue<Option<T>>> {
        ThreadPool::fixed_size(1)
    }
}

impl<T: Task+'static, Q: WorkQueue<T>> ThreadPool<T, Q> {
    pub fn new(core_pool_size: u32,
               maximum_pool_size: u32,
               work_queue: Q) -> ThreadPool<T, Q> {

        let inner = ThreadPoolInner::new(
            core_pool_size,
            maximum_pool_size,
            work_queue);

        ThreadPool { inner: Arc::new(inner) }
    }

    pub fn prestart_core_thread(&self) {
        self.inner.prestart_core_thread();
    }

    pub fn prestart_all_core_threads(&self) {
        self.inner.prestart_all_core_threads();
    }

    pub fn shutdown(&self) {
        self.inner.shutdown(Lifecycle::Shutdown);
    }

    pub fn shutdown_now(&self) {
        self.inner.shutdown(Lifecycle::Stop);
    }

    pub fn is_shutdown(&self) -> bool {
        self.inner.is_shutdown()
    }

    pub fn await_termination(&self) {
        self.inner.await_termination();
    }
}

impl<T: Task+'static, Q: WorkQueue<T>> Run<T> for ThreadPool<T, Q> {
    fn run(&self, task: T) {
        self.inner.run(task, true);
    }
}

impl<T: Task+'static, Q: WorkQueue<T>> Clone for ThreadPool<T, Q> {
    fn clone(&self) -> ThreadPool<T, Q> {
        ThreadPool { inner: self.inner.clone() }
    }
}

/// A thread pool that can schedule tasks to run after a given delay, or to
/// execute periodically. Delayed tasks do not run before their associated
/// delays, but besides that, there are no real-time guarantees about exactly
/// when the task will run.
pub struct ScheduledThreadPool {
    thread_pool: ThreadPool<Scheduled, DelayQueue<Option<Scheduled>>>,
}

impl ScheduledThreadPool {
    pub fn fixed_size(size: u32) -> ScheduledThreadPool {
        assert!(size > 0, "thread pool size must be greater than 0");

        ScheduledThreadPool {
            thread_pool: ThreadPool::new(size, size, DelayQueue::new())
        }
    }

    pub fn single_thread() -> ScheduledThreadPool {
        ScheduledThreadPool::fixed_size(1)
    }

    pub fn schedule_ms<T: Task+'static>(&self, delay: u32, task: T) {
        let delay = Duration::milliseconds(delay as i64);
        let task = Scheduled::Delayed(Box::new(task), delay);

        self.thread_pool.inner.run(task, false);
    }
}

impl<T: Task+'static> Run<T> for ScheduledThreadPool {
    fn run(&self, task: T) {
        self.schedule_ms(0, task);
    }
}

impl Clone for ScheduledThreadPool {
    fn clone(&self) -> ScheduledThreadPool {
        ScheduledThreadPool { thread_pool: self.thread_pool.clone() }
    }
}

enum Scheduled {
    Delayed(Box<BoxedTask>, Duration),
}

impl Delayed for Scheduled {
    fn delay(&self) -> Duration {
        match *self {
            Scheduled::Delayed(_, d) => d,
        }
    }
}

impl Task for Scheduled {
    fn run(self) {
        debug!("~~ running scheduled task");
        match self {
            Scheduled::Delayed(t, _) => t.run_boxed(),
        }
    }
}

trait BoxedTask : Task + 'static {
    fn run_boxed(self: Box<Self>);
}

impl<T: Task + 'static> BoxedTask for T {
    fn run_boxed(self: Box<T>) {
        let s = *self;
        s.run();
    }
}

// ## Notes
//
// It's important that a worker increments the count before pulling from the
// queue and never touches the queue after decrementing the worker count.
//
// ## TODO
//
// - Poison the thread pool if something goes critically wrong
//
struct ThreadPoolInner<T: Task+'static, Q: WorkQueue<T>> {

    // Contains the state, condvar, etc..
    core: Arc<Core>,

    // The queue used for holding tasks and handing off to worker
    // threads. We do not require that worker_queue.poll() returning
    // None necessarily means that the queue is empty, so rely
    // solely on is_empty to see if the queue is empty (which we must
    // do for example when deciding whether to transition from
    // Shutdown to Tidying).  This accommodates special-purpose
    // queues such as DelayQueue for which poll() is allowed to return null
    // even if it may later return non-null when delays expire.
    work_queue: Q,
    task: PhantomData<T>,
}

impl<T: Task+'static, Q: WorkQueue<T>> ThreadPoolInner<T, Q> {

    fn new(core_pool_size: u32,
           maximum_pool_size: u32,
           work_queue: Q) -> ThreadPoolInner<T, Q> {

        let core = Arc::new(Core::new(core_pool_size, maximum_pool_size));

        ThreadPoolInner {
            core: core,
            work_queue: work_queue,
            task: PhantomData,
        }
    }

    fn prestart_core_thread(&self) {
        let _ = self.add_worker(None, true);
    }

    fn prestart_all_core_threads(&self) {
        loop {
            if self.add_worker(None, true).is_err() {
                return;
            }
        }
    }

    fn run(&self, mut task: T, immediate: bool) {
        //  Proceed in 3 steps:
        //
        //  1. If fewer than `core_pool_size` threads are running, try to
        //  start a new thread with the given task as its first
        //  task.  The call to `add_worker` atomically checks `lifecycle` and
        //  `worker_count`, and so prevents false alarms that would add
        //  threads when it shouldn't, by returning false.
        //
        //  2. If a task can be successfully queued, then we still need
        //  to double-check whether we should have added a thread
        //  (because existing ones died since last checking) or that
        //  the pool shut down since entry into this method. So we
        //  recheck state and if necessary roll back the enqueuing if
        //  stopped, or start a new thread if there are none.
        //
        //  3. If we cannot queue task, then we try to add a new
        //  thread.  If it fails, we know we are shut down or saturated
        //  and so reject the task.

        let mut state = self.core.state.load(Ordering::Relaxed);

        debug!("running task; tp-state={:?}; worker_count={}",
               state.lifecycle(), state.worker_count());

        // An optimization allowing spawning of a worker through with a
        // specified task
        if state.worker_count() < self.core.core_pool_size {
            if immediate {
                match self.add_worker(Some(task), true) {
                    Ok(_) => {
                        debug!("worker successfully added; core=true");
                        return
                    }
                    Err(t) => task = t.expect("something went wrong"),
                }
            } else {
                // Simply ensure that there are enough running threads
                let _ = self.add_worker(None, false);
            }

            state = self.core.state.load(Ordering::Relaxed);
        }

        if state.is_running() {
            // The current state is running, attempt to place the task on the
            // queue. If this fails, the queue is full (or is in some other
            // error condition). Return the task to the caller.
            if let Err(t) = self.work_queue.offer(Some(task)) {
                task = t.expect("something went wrong");

                debug!("failed to push task onto queue -- attempting to add worker");
                // The queue is full, attempt to grow the pool
                match self.add_worker(Some(task), false) {
                    Ok(_) => {
                        debug!("worker successfully added; core=false");
                        return
                    },
                    Err(_task) => {
                        warn!("failed to submit task to worker");
                        // TODO: return the task to the caller
                        return;
                    }
                }
            }

            debug!("task submitted to queue");

            return;
        }

        debug!("threadpool is not accepting new tasks");
    }

    fn shutdown(&self, target: Lifecycle) {
        // Transition from Running -> Shutdown
        let mut state = self.core.state.load(Ordering::Relaxed);
        let mut next;

        debug!("shutdown; tp-state={:?}; target={:?}; worker_count={}",
               state.lifecycle(), target, state.worker_count());

        loop {
            next = match state.lifecycle() {
                Lifecycle::Running=> {
                    if state.worker_count() == 0 {
                        state.with_lifecycle(Lifecycle::Terminated)
                    } else {
                        state.with_lifecycle(target)
                    }
                }
                Lifecycle::Shutdown=> {
                    if target == Lifecycle::Shutdown {
                        return;
                    }

                    state.with_lifecycle(target)
                }
                _ => return,
            };

            let actual = self.core.state.compare_and_swap(state, next, Ordering::Relaxed);

            if actual == state {
                break;
            }

            state = actual;
        }

        if next.is_terminated() {
            debug!("  transitioned directly to terminated");
            return;
        }

        // Submit bogus messages to wakeup the workers (they will then check
        // the state and act on it)
        let mut i = 0;
        let cnt = next.worker_count();

        debug!("  enqueuing no-ops; count={}", cnt);

        while i < cnt {
            // Enqueue a no-op for each worker. This allows the worker to
            // unblock if it is currently waiting for a task. Once the worker
            // sees the no-op it will not pull from the queue again.
            self.work_queue.put(None);
            i += 1;
        }

    }

    fn is_shutdown(&self) -> bool {
        !self.core.state.load(Ordering::Relaxed).is_running()
    }

    fn await_termination(&self) {
        self.core.await_termination();
    }

    fn add_worker(&self, task: Option<T>, core: bool)
            -> Result<(), Option<T>> {

        // == Transition the state ==

        let mut state = self.core.state.load(Ordering::Relaxed);

        'retry: loop {
            let lifecycle = state.lifecycle();

            if lifecycle >= Lifecycle::Stop {
                // If the lifecycle is greater than Lifecycle::Stop than never
                // create a add a new worker
                return Err(task);
            }

            if lifecycle == Lifecycle::Shutdown {
                // If the lifecycle is currently Shutdown, only add a new
                // worker if it is needed (there is work to process).
                if task.is_none() && self.work_queue.is_empty() {
                    return Err(task);
                }
            }

            loop {
                let wc = state.worker_count();

                // The number of threads that are expected to be running
                let target = if core { self.core.core_pool_size } else { self.core.maximum_pool_size };

                if wc >= CAPACITY || wc >= target {
                    return Err(task);
                }

                if self.core.state.compare_and_inc_worker_count(state, Ordering::Relaxed) {
                    break 'retry;
                }

                // CAS failed, re-read state
                state = self.core.state.load(Ordering::Relaxed);

                if state.lifecycle() != lifecycle {
                    continue 'retry;
                }

                // CAS failed due to worker_count change; retry inner loop
            }
        }

        // == Spawn the thread ==

        let mut worker = Worker::new(
            self.core.clone(), task,
            self.work_queue.clone());

        debug!("spawning new worker thread");

        thread::spawn(move || worker.run());

        Ok(())
    }
}

impl<T: Task+'static, Q: WorkQueue<T>> Drop for ThreadPoolInner<T, Q> {
    fn drop(&mut self) {
        self.shutdown(Lifecycle::Shutdown);
    }
}

// Needed because of the PhantomData marker
unsafe impl<T: Task+'static, Q: WorkQueue<T>> Send for ThreadPoolInner<T, Q> { }
unsafe impl<T: Task+'static, Q: WorkQueue<T>> Sync for ThreadPoolInner<T, Q> { }

struct Worker<T: Task+'static, Q: WorkQueue<T>> {
    // Core shared by ThreadPool and Worker
    core: Arc<Core>,

    // The task to run when the thread first starts
    initial_task: Option<T>,

    // The queue on which to listen for new tasks
    work_queue: Q,

    // Checked in the drop function whether or not the thread panicked
    panicked: bool,
}

impl<T: Task+'static, Q: WorkQueue<T>> Worker<T, Q> {
    fn new(core: Arc<Core>, initial_task: Option<T>, queue: Q) -> Worker<T, Q> {
        Worker {
            core: core,
            initial_task: initial_task,
            work_queue: queue,
            panicked: false,
        }
    }

    fn run(&mut self) {
        self.panicked = true;

        while let Some(task) = self.get_task() {
            debug!("worker processing task");
            task.run();
        }

        self.panicked = false;
    }

    // Gets the next task, blocking if necessary. Returns None if the worker
    // should shutdown
    fn get_task(&mut self) -> Option<T> {
        let mut task = self.initial_task.take();

        // Load the state
        let state = self.core.state.load(Ordering::Relaxed);

        loop {
            if state.lifecycle() >= Lifecycle::Stop {
                debug!("threadpool is stopped -- aborting task get");

                // No more tasks should be removed from the queue, exit the
                // worker
                self.decrement_worker_count(false);

                // Nothing else to do
                return None;
            }

            if task.is_some() {
                break;
            }

            let wc = state.worker_count();

            if wc > self.core.maximum_pool_size {
                if self.core.state.compare_and_dec_worker_count(state, Ordering::Relaxed) {
                    debug!("threadpool over max worker count -- shutting thread down; count={}", wc);

                    // This can never be a termination state since the
                    // lifecycle is not Stop or Terminate (checked above) and
                    // the queue has not been accessed, so it is unknown
                    // whether or not there is a pending task.
                    //
                    // This means that there is no need to call
                    // `finalize_worker`
                    return None;
                }

                // CAS failed, restart loop
                continue;
            }

            debug!("worker waiting for task");
            match self.work_queue.take() {
                Some(t) => {
                    // Grab the task, but the loop will restart in order to
                    // check the state again. If the state transitioned to Stop
                    // while the worker was blocked on the queue, the task
                    // should be discarded and the worker shutdown.
                    task = Some(t);
                }
                None => {
                    debug!("received no-op token -- shutting down");
                    // No more tasks should be removed from the queue, exit the
                    // worker
                    self.decrement_worker_count(false);

                    // Nothing else to do
                    return None;
                }
            }
        }

        task
    }

    fn decrement_worker_count(&self, panicking: bool) {
        let state = self.core.state.fetch_dec_worker_count(Ordering::Relaxed);

        if state.worker_count() == 1 {
            if panicking {
                if state.lifecycle() >= Lifecycle::Stop {
                    let core = self.core.clone();

                    thread::spawn(move || {
                        core.finalize_threadpool();
                    });
                }
            } else {
                self.core.finalize_threadpool();
            }
        }
    }
}

impl<T: Task+'static, Q: WorkQueue<T>> Drop for Worker<T, Q> {
    fn drop(&mut self) {
        if self.panicked {
            self.decrement_worker_count(true);
        }
    }
}

struct Core {
    // The main pool control state is an atomic integer packing two conceptual
    // fields
    //   worker_count: indicating the effective number of threads
    //   lifecycle:    indicating whether running, shutting down etc
    //
    // In order to pack them into one i32, we limit `worker_count` to (2^29)-1
    // (about 500 million) threads rather than (2^31)-1 (2 billion) otherwise
    // representable.
    //
    // The `worker_count` is the number of workers that have been permitted to
    // start and not permitted to stop. The value may be transiently different
    // from the actual number of live threads, for example when a thread
    // spawning fails to create a thread when asked, and when exiting threads
    // are still performing bookkeeping before terminating. The user-visible
    // pool size is reported as the current size of the workers set.
    //
    // The `lifecycle` provides the main lifecyle control, taking on values:
    //
    //   Running:    Accept new tasks and process queued tasks
    //   Shutdown:   Don't accept new tasks, but process queued tasks
    //   Stop:       Don't accept new tasks, don't process queued tasks, and
    //               interrupt in-progress tasks
    //   Tidying:    All tasks have terminated, worker_count is zero, the thread
    //               transitioning to state Tidying will run the terminated() hook
    //               method
    //   Terminated: terminated() has completed
    //
    // The numerical order among these values matters, to allow ordered
    // comparisons. The lifecycle monotonically increases over time, but need
    // not hit each state. The transitions are:
    //
    //   Running -> Shutdown
    //      On invocation of shutdown(), perhaps implicitly in finalize()
    //
    //   (Running or Shutdown) -> Stop
    //      On invocation of shutdown_now()
    //
    //   Shutdown -> Tidying
    //      When both queue and pool are empty
    //
    //   Stop -> Tidying
    //      When pool is empty
    //
    //   Tidying -> Terminated
    //      When the terminated() hook method has completed
    //
    // Threads waiting in await_termination() will return when the state reaches
    // Terminated.
    //
    // Detecting the transition from Shutdown to Tidying is less
    // straightforward than you'd like because the queue may become empty after
    // non-empty and vice versa during Shutdown state, but we can only
    // terminate if, after seeing that it is empty, we see that workerCount is
    // 0 (which sometimes entails a recheck -- see below).
    state: AtomicState,

    mutex: Mutex<()>,

    // Wait condition to support awaitTermination
    termination: Condvar,

    // Core pool size is the minimum number of workers to keep alive
    // (and not allow to time out etc) unless allowCoreThreadTimeOut
    // is set, in which case the minimum is zero.
    core_pool_size: u32,

    // Maximum pool size. Note that the actual maximum is internally
    // bounded by CAPACITY.
    maximum_pool_size: u32,
}

impl Core {
    fn new(core_pool_size: u32, maximum_pool_size: u32) -> Core {
        Core {
            state: AtomicState::new(Lifecycle::Running),
            mutex: Mutex::new(()),
            termination: Condvar::new(),
            core_pool_size: core_pool_size,
            maximum_pool_size: maximum_pool_size,
        }
    }

    fn await_termination(&self) {
        let mut lock = self.mutex.lock()
            .ok().expect("something went wrong");

        loop {
            if self.state.load(Ordering::Relaxed).is_terminated() {
                return;
            }

            lock = self.termination.wait(lock)
                .ok().expect("something went wrong");
        }
    }

    fn finalize_threadpool(&self) {
        let _lock = self.mutex.lock()
            .ok().expect("something went wrong");

        // Transition to Terminated
        self.state.transition_to_terminated(Ordering::Relaxed);

        // Notify all pending threads
        self.termination.notify_all();
    }
}

struct AtomicState {
    atomic: AtomicUsize,
}

impl AtomicState {
    fn new(lifecycle: Lifecycle) -> AtomicState {
        let i = State::of(lifecycle).as_u32();

        AtomicState {
            atomic: AtomicUsize::new(i as usize),
        }
    }

    fn load(&self, order: Ordering) -> State {
        let num = self.atomic.load(order);
        State::load(num as u32)
    }

    fn compare_and_swap(&self, expect: State, val: State, order: Ordering) -> State {
        let actual = self.atomic.compare_and_swap(expect.as_usize(), val.as_usize(), order);
        State::load(actual as u32)
    }

    fn compare_and_inc_worker_count(&self, expect: State, order: Ordering) -> bool {
        let num = expect.as_usize();
        self.atomic.compare_and_swap(num, num + (1 << LIFECYCLE_BITS), order) == num
    }

    fn compare_and_dec_worker_count(&self, expect: State, order: Ordering) -> bool {
        if expect.worker_count() == 0 {
            panic!("something went wrong");
        }

        let num = expect.as_usize();
        self.atomic.compare_and_swap(num, num - (1 << LIFECYCLE_BITS), order) == num
    }

    fn fetch_dec_worker_count(&self, order: Ordering) -> State {
        let prev = self.atomic.fetch_sub(1 << LIFECYCLE_BITS, order);
        State::load(prev as u32)
    }

    fn transition_to_terminated(&self, order: Ordering) {
        let mut state = self.load(order);

        loop {
            let next = state.with_lifecycle(Lifecycle::Terminated);
            let actual = self.compare_and_swap(state, next, order);

            if state == actual {
                return;
            }

            state = actual;
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct State {
    state: u32,
}

impl State {
    fn load(num: u32) -> State {
        State { state: num }
    }

    fn of(lifecycle: Lifecycle) -> State {
        State { state: lifecycle as u32 }
    }

    fn lifecycle(&self) -> Lifecycle {
        Lifecycle::from_u32(self.state & LIFECYCLE_MASK)
    }

    fn with_lifecycle(&self, lifecycle: Lifecycle) -> State {
        let state = self.state & !LIFECYCLE_MASK | lifecycle as u32;
        State { state: state }
    }

    fn worker_count(&self) -> u32 {
        self.state >> LIFECYCLE_BITS
    }

    fn is_running(&self) -> bool {
        self.lifecycle() == Lifecycle::Running
    }

    fn is_terminated(&self) -> bool {
        self.lifecycle() == Lifecycle::Terminated
    }

    fn as_u32(&self) -> u32 {
        self.state
    }

    fn as_usize(&self) -> usize {
        self.state as usize
    }
}

const LIFECYCLE_BITS: u32 = 3;
const LIFECYCLE_MASK: u32 = 7;
const CAPACITY: u32 = (1 << (32 - 3)) - 1;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum Lifecycle {
    Running    = 0,
    Shutdown   = 1,
    Stop       = 2,
    Tidying    = 3,
    Terminated = 4,
}

impl Lifecycle {
    fn from_u32(val: u32) -> Lifecycle {
        use self::Lifecycle::*;

        match val {
            0 => Running,
            1 => Shutdown,
            2 => Stop,
            3 => Tidying,
            4 => Terminated,
            _ => panic!("unexpected state value"),
        }
    }
}
