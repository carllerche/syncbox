use {sleep_ms};
use syncbox::util::ThreadPool;
use std::sync::mpsc::*;

#[test]
pub fn test_one_thread_basic() {
    let tp = ThreadPool::single_thread();
    let (tx, rx) = sync_channel(0);

    tp.run(move || {
        tx.send("hi").unwrap();
    });

    assert_eq!("hi", rx.recv().unwrap());
}

#[test]
pub fn test_two_thread_basic() {
    let tp = ThreadPool::fixed_size(2);
    let (tx, rx) = sync_channel(0);

    for i in (0..2i32) {
        let tx = tx.clone();
        tp.run(move || {
            debug!("send; task={}; msg=hi", i);
            tx.send("hi").unwrap();
            sleep_ms(50);

            debug!("send; task={}; msg=bye", i);
            tx.send("bye").unwrap();
            sleep_ms(50);
        });
    }

    debug!("recv");

    for &msg in ["hi", "hi", "bye", "bye"].iter() {
        assert_eq!(msg, rx.recv().unwrap());
    }
}

#[test]
pub fn test_two_threads_task_queue_up() {
    let tp = ThreadPool::fixed_size(2);
    let (tx, rx) = sync_channel(0);

    for i in (0..4i32) {
        let tx = tx.clone();
        tp.run(move || {
            debug!("send; task={}; msg=hi", i);
            tx.send("hi").unwrap();
            sleep_ms(50);

            debug!("send; task={}; msg=bye", i);
            tx.send("bye").unwrap();
            sleep_ms(50);
        });
    }

    debug!("recv");

    for &msg in ["hi", "hi", "bye", "bye", "hi", "hi", "bye", "bye"].iter() {
        assert_eq!(msg, rx.recv().unwrap());
    }
}
