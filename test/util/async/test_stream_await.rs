use syncbox::util::async::*;

#[test]
#[should_panic]
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

#[test]
pub fn test_stream_fail_await() {
    let (tx, rx) = Stream::<u32, &'static str>::pair();

    tx.fail("nope");
    assert_eq!("nope", rx.await().unwrap_err().unwrap());
}

#[test]
pub fn test_stream_fail_second_iter_await() {
    let (tx, mut rx) = Stream::<u32, &'static str>::pair();

    tx.send(1)
        .and_then(|tx| tx.fail("nope"))
        .fire();

    match rx.await() {
        Ok(Some((v, rest))) => {
            assert_eq!(v, 1);
            rx = rest;
        }
        _ => panic!("unexpected value for head of stream"),
    }

    assert_eq!("nope", rx.await().unwrap_err().unwrap());
}
