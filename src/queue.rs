// TODO:
// - Consider splitting up the trait into Consume / Produce.
// - Break up SyncQueue from Queue
pub trait Queue<T: Send> {

    /// Retrieves and removes the head of this queue or returns None if the
    /// queue is empty.
    fn poll(&self) -> Option<T>;

    /// Returns true if the underlying data structure does not contain any
    /// elements.
    fn is_empty(&self) -> bool;

    /// Adds the element `e` to the queue if possible.
    ///
    /// # Errors
    ///
    /// A call to `offer` will fail if the queue is full; the provided element
    /// `e` is returned in the `Err` variant.
    fn offer(&self, e: T) -> Result<(), T>;
}

pub trait SyncQueue<T: Send> : Queue<T> {
    /// Retrieves and removes the head of this queue, waiting if necessary
    /// until an element becomes available.
    fn take(&self) -> T;

    /// Adds the element `e` to the queue, blocking until it can be added.
    fn put(&self, e: T);
}
