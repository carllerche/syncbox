use syncbox::util::async::*;
use std::sync::mpsc::{self, channel};
use super::{nums};

#[test]
pub fn test_stream_map_async() {
    let s = nums(0, 5).map(move |i| 2 * i);
    let (tx, rx) = channel();

    fn receive(s: Stream<i32, ()>, tx: mpsc::Sender<i32>) {
        debug!("Stream::receive");
        s.receive(move |res| {
            res.map(move |head| {
                head.map(move |(v, rest)| {
                    tx.send(v).unwrap();
                    receive(rest, tx);
                });
            }).unwrap();
        });
    }

    receive(s, tx);

    let vals: Vec<i32> = rx.iter().collect();
    assert_eq!([0, 2, 4, 6, 8].as_slice(), vals.as_slice());
}
