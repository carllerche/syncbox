use syncbox::util::async::*;
use std::sync::mpsc::*;
use super::{spawn, sleep};

#[test]
pub fn test_one_shot_stream_async() {
}

#[test]
pub fn test_one_shot_stream_await() {
    let (stream, producer) = Stream::<&'static str, ()>::pair();
    let (tx, rx) = channel();

    spawn(move || {
        debug!(" ~~ awaiting on stream ~~");
        match stream.await() {
            Ok(Some((h, _))) => tx.send(h).unwrap(),
            _ => panic!("nope"),
        }
    });

    sleep(50);

    debug!(" ~~ Generate::send(\"hello\") ~~");
    producer.send("hello");
    assert_eq!("hello", rx.recv().unwrap());
}

#[test]
pub fn test_one_shot_stream_done() {
    let (mut stream, produce) = Stream::<&'static str, ()>::pair();
    let (tx, rx) = channel();

    spawn(move || {
        while let Ok(Some((v, rest))) = stream.await() {
            tx.send(v).unwrap();
            stream = rest;
        }
    });

    produce.send("hello");
    produce.done();

    let vals: Vec<&'static str> = rx.iter().collect();
    assert_eq!(["hello"].as_slice(), vals.as_slice());
}

#[test]
pub fn test_stream_receive_before_produce_interest_async() {
    let (stream, producer) = Stream::pair();
    let (tx, rx) = channel();
    let (eof_t, eof_r) = channel();

    fn do_receive(stream: Stream<&'static str, ()>, tx: Sender<&'static str>, eof: Sender<bool>) {
        stream.receive(move |res| {
            match res {
                Ok(Some((h, rest))) => {
                    tx.send(h).unwrap();
                    do_receive(rest, tx, eof);
                }
                Ok(None) => eof.send(true).unwrap(),
                _ => panic!("unexpected error"),
            }
        });
    }

    do_receive(stream, tx, eof_t);

    // Transfer the producer out
    let (txp, rxp) = channel();

    debug!(" ~~ Generate::receive ~~");
    producer.receive(move |p| {
        let p = p.unwrap();
        p.send("hello");
        txp.send(p).unwrap();
    });

    // Receive the first message
    assert_eq!("hello", rx.recv().unwrap());

    // New channel to transfer the producer out
    let (txp, rxp2) = channel();

    // Get the producer, wait for readiness, and write another message
    debug!(" ~~ Generate::receive ~~");
    rxp.recv().unwrap().receive(move |p| {
        let p = p.unwrap();
        p.send("world");
        txp.send(p).unwrap();
    });

    debug!(" ~~ WAITING ON WORLD ~~");

    // Receive the second message
    assert_eq!("world", rx.recv().unwrap());

    debug!(" ~~ Generate::receive ~~");
    rxp2.recv().unwrap().receive(move |p| {
        p.unwrap().done();
    });

    debug!(" ~~ Waiting on None ~~");
    assert!(eof_r.recv().unwrap());
    assert!(rx.recv().is_err());
}

#[test]
pub fn test_stream_produce_interest_before_receive_async() {
    let (mut stream, producer) = Stream::<&'static str, ()>::pair();
    let (txp, rxp) = channel();

    debug!(" ~~ Generate::receive #1 ~~");
    producer.receive(move |p| {
        let p = p.unwrap();
        p.send("hello");
        txp.send(p).unwrap();
    });

    let (tx, rx) = channel();

    debug!(" ~~ Stream::receive #1 ~~");
    stream.receive(move |res| tx.send(res).unwrap());

    match rx.recv().unwrap() {
        Ok(Some((v, rest))) => {
            assert_eq!(v, "hello");
            stream = rest;
        }
        _ => panic!("nope"),
    }

    let (txp2, rxp2) = channel();

    debug!(" ~~ Generate::receive #2 ~~");
    rxp.recv().unwrap().receive(move |p| {
        let p = p.unwrap();
        p.send("world");
        txp2.send(p).unwrap();
    });

    let (tx, rx) = channel();

    debug!(" ~~ Stream::receive #2 ~~");
    stream.receive(move |res| tx.send(res).unwrap());

    match rx.recv().unwrap() {
        Ok(Some((v, rest))) => {
            assert_eq!(v, "world");
            stream = rest;
        }
        _ => panic!("nope"),
    }

    rxp2.recv().unwrap().receive(move |p| p.unwrap().done());

    let (tx, rx) = channel();

    debug!(" ~~ Stream::receive #3 ~~");
    stream.receive(move |res| tx.send(res).unwrap());

    match rx.recv().unwrap() {
        Ok(None) => {}
        _ => panic!("nope"),
    }
}

#[test]
pub fn test_stream_produce_before_receive_async() {
}

#[test]
pub fn test_stream_send_then_done_before_receive() {
    // Currently, Generate::done() cannot be called before the previous
    // value is consumed. It would be ideal to be able to signal that the
    // stream is done without having to do another producer callback
    // iteration.
}

#[test]
pub fn test_recursive_receive() {
    let (s, p) = Stream::<uint, ()>::pair();
    let (tx, rx) = channel();

    fn consume(s: Stream<uint, ()>, tx: Sender<uint>) {
        debug!(" ~~~~ CONSUME ENTER ~~~~~ ");
        s.receive(move |res| {
            debug!(" ~~~ CONSUME CALLBACK ENTER ~~~~~ ");
            if let Some((v, rest)) = res.unwrap() {
                tx.send(v).unwrap();
                consume(rest, tx);
            }
            debug!(" ~~~ CONSUME CALLBACK EXIT ~~~~~ ");
        });
        debug!(" ~~~~ CONSUME EXIT ~~~~~ ");
    }

    fn produce(p: Generate<uint, ()>, n: uint) {
        debug!(" ~~~~ PRODUCE ENTER ~~~~~ ");
        if n > 20_000 {
            p.done();
            return;
        }

        p.receive(move |res| {
            debug!(" ~~~ PROD CALLBACK ENTER ~~~~ ");
            let p = res.unwrap();
            p.send(n);
            produce(p, n + 1);
            debug!(" ~~~ PROD CALLBACK EXIT ~~~~ ");
        });
        debug!(" ~~~~ PRODUCE EXIT ~~~~~ ");
    }

    consume(s, tx);
    produce(p, 1);

    let vals: Vec<uint> = rx.iter().collect();
    assert_eq!(vals.len(), 20_000);
}

#[test]
#[ignore]
pub fn test_complete_during_consumer_receive() {
    // The producer completes the next stream iteration during the receiver's
    // callback
}

#[test]
#[ignore]
pub fn test_recurse_and_cancel() {
    // unimplemented
}
