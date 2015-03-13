use syncbox::util::async::*;
use super::nums;
use syncbox::util::async::AsyncError::{ExecutionError};

#[test]
pub fn test_stream_reduce_async() {
    let s = nums(0, 5).reduce(10, move |sum, v| sum + v);
    assert_eq!(20, s.await().unwrap());
}

#[test]
pub fn test_stream_reduce_fail() {
    let (tx, rx) = Stream::pair();
    tx.send(1)
        .and_then(|tx| tx.send(2))
        .and_then(|tx| tx.send(3))
        .and_then(|tx| tx.fail(()))
        .receive(drop);

    let reduced = rx.reduce(0, move |sum, v| sum + v);
    assert_eq!(Err(ExecutionError(())), reduced.await());
}
