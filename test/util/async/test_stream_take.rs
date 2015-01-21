use super::nums;

#[test]
pub fn test_stream_take() {
    let stream = nums(0, 10).take(4);
    let vals: Vec<usize> = stream.iter().collect();

    assert_eq!([0, 1, 2, 3].as_slice(), vals.as_slice());
}
