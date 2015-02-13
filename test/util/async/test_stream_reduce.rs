use syncbox::util::async::*;
use super::nums;

#[test]
pub fn test_stream_reduce_async() {
    let s = nums(0, 5).reduce(10, move |sum, v| sum + v);
    assert_eq!(20, s.await().unwrap());
}
