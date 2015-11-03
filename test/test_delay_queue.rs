use syncbox::*;
use time::Duration;

#[test]
fn test_ordering() {
    let queue = DelayQueue::new();

    queue.offer(Delay(1i32, Duration::milliseconds(30))).unwrap();
    queue.offer(Delay(2i32, Duration::milliseconds(10))).unwrap();
    queue.offer(Delay(3i32, Duration::milliseconds(20))).unwrap();

    assert_eq!(2, *queue.take());
    assert_eq!(3, *queue.take());
    assert_eq!(1, *queue.take());
}

#[test]
fn test_poll() {
    let queue = DelayQueue::new();

    queue.offer(Delay(1i32, Duration::nanoseconds(0))).unwrap();
    queue.offer(Delay(2i32, Duration::days(1))).unwrap();

    assert_eq!(1, *queue.poll().unwrap());
    assert_eq!(None, queue.poll());
}

#[test]
fn test_poll_timeout() {
    let queue = DelayQueue::new();

    queue.offer(Delay(1i32, Duration::nanoseconds(0))).unwrap();
    queue.offer(Delay(2i32, Duration::milliseconds(250))).unwrap();
    queue.offer(Delay(3i32, Duration::days(1))).unwrap();

    assert_eq!(1, *queue.poll_timeout(Duration::milliseconds(250)).unwrap());
    assert_eq!(2, *queue.poll_timeout(Duration::milliseconds(500)).unwrap());
    assert_eq!(None, queue.poll_timeout(Duration::milliseconds(500)));
}
