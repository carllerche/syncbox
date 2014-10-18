use std::time::Duration;

pub trait Consume<T> {
    /// Retrieves and removes the head of the queue.
    fn take(&self) -> Option<T> {
        self.take_wait(Duration::milliseconds(0))
    }

    /// Retrieves and removes the head of the queue, waiting if necessary up to
    /// the specified wait time.
    fn take_wait(&self, timeout: Duration) -> Option<T>;
}

pub trait Produce<T> {
    /// Inserts the value into the queue.
    fn put(&self, val: T) -> Result<(), T> {
        self.put_wait(val, Duration::milliseconds(0))
    }

    /// Inserts the value into the queue, waiting if necessary up to the
    /// specified wait time.
    fn put_wait(&self, val: T, timeout: Duration) -> Result<(), T>;
}
