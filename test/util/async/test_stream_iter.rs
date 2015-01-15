use syncbox::util::async::*;

#[test]
pub fn test_stream_iter_async_producer() {
    let (stream, producer) = Stream::pair();

    fn gen(p: Produce<uint, ()>, i: uint) {
        p.receive(move |p| {
            let p = p.unwrap();

            if i < 5 {
                p.send(i);
                gen(p, i + 1);
            } else {
                p.done();
            }
        });
    }

    gen(producer, 0);

    let vals: Vec<uint> = stream.iter().collect();
    assert_eq!([0, 1, 2, 3, 4].as_slice(), vals.as_slice());
}
