use syncbox::util::async::*;

#[test]
#[should_fail]
pub fn test_await_in_receive() {
    debug!("starting");

    let (s, p) = Stream::<uint, ()>::pair();

    s.receive(move |res| {
        if let Some((_, rest)) = res.unwrap() {
            // Calling await from within a receive callback creates a deadlock
            // situation when there only is a single thread involved. To avoid
            // this, panic! is called from the await.
            let _ = rest.await();
        }
    });

    fn produce(p: Produce<uint, ()>, n: uint) {
        if n > 100_000 { return; }

        p.receive(move |res| {
            let p = res.unwrap();
            p.send(n);
            produce(p, n + 1);
        });
    }

    produce(p, 1);
}
