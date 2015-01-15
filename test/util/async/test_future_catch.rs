use syncbox::util::async::*;
use std::sync::mpsc::*;

#[test]
pub fn test_complete_before_receive() {
    let (f, c) = Future::<&'static str, i32>::pair();
    let (tx, rx) = channel();

    f.catch(move |e| {
        assert_eq!(123, e.unwrap());
        Ok::<&'static str, AsyncError<()>>("caught")
    }).receive(move |res| {
        tx.send(res.unwrap()).unwrap();
    });

    c.fail(123);

    assert_eq!(rx.recv().unwrap(), "caught");
}
