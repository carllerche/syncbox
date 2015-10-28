// TODO:
// - Consider splitting up the trait into Consume / Produce.
// - Break up SyncQueue from Queue

/// An interface for queues of elements of type `T`.
pub trait Queue<T: Send> {

    /// Takes from the queue if there is an element available.
    fn poll(&self) -> Option<T>;

    /// Returns `true` if the queue contains no elements.
    fn is_empty(&self) -> bool;

    /// Adds the element `e` to the queue if possible.
    ///
    /// # Errors
    ///
    /// A call to `offer` will fail if the queue is full; the provided element
    /// `e` is returned in the `Err` variant.
    fn offer(&self, e: T) -> Result<(), T>;
}

/// An interface for synchronous queues of elements of type `T`.
///
/// This extends `Queue` with blocking analogs of `poll` and `offer`. Both
/// `take` and `put` will block the calling thread until the operation
/// succeeds.
pub trait SyncQueue<T: Send> : Queue<T> {
    /// Takes from the queue, blocking until there is an element available.
    fn take(&self) -> T;

    /// Adds the element `e` to the queue, blocking until it can be added.
    fn put(&self, e: T);
}
