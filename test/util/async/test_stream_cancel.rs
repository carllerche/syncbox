use syncbox::util::async::*;
use std::sync::mpsc::*;

#[test]
pub fn test_stream_cancel_before_send() {
    let (stream, generate) = Stream::<i32, ()>::pair();

    let cancel = stream.ready(move |_| panic!("nope"));
    let stream = cancel.cancel().expect("cancel failed");

    generate.send(123);

    match stream.expect().unwrap() {
        Some((v, _)) => assert_eq!(123, v),
        _ => panic!("nope"),
    }
}

#[test]
pub fn test_stream_cancel_after_send() {
    let (stream, generate) = Stream::<i32, ()>::pair();
    let (tx, rx) = channel();

    let cancel = stream.ready(move |s| {
        tx.send(s.expect().unwrap()).unwrap()
    });

    generate.send(123);

    assert!(cancel.cancel().is_none());

    match rx.recv().unwrap() {
        Some((v, _)) => assert_eq!(123, v),
        _ => panic!("nope"),
    }
}
