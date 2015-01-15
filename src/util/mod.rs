pub use std::sync::atomic;
pub use self::linked_queue::LinkedQueue;

pub mod async;
mod linked_queue;

pub trait Consume<T: Send> {
    fn poll(&self) -> Option<T>;

    fn take(&self) -> T;
}
