use syncbox::util::async::*;

#[test]
#[should_fail]
pub fn test_await_in_receive() {
    debug!("starting");

    let (tx, rx) = Stream::<uint, ()>::pair();

    rx.receive(move |res| {
        if let Some((_, rest)) = res.unwrap() {
            // Calling await from within a receive callback creates a deadlock
            // situation when there only is a single thread involved. To avoid
            // this, panic! is called from the await.
            let _ = rest.await();
        }
    });

    fn produce(tx: Sender<uint, ()>, n: uint) {
        if n > 100_000 { return; }

        tx.receive(move |res| {
            let tx = res.unwrap();
            tx.send(n).and_then(move |tx| produce(tx, n + 1));
        });
    }

    produce(tx, 1);
}
