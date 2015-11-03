use {spawn, sleep_ms};
use syncbox::LinkedQueue;
use time;
use std::thread;

#[test]
pub fn test_single_threaded_put_take() {
    let q = LinkedQueue::new();

    // Check the length
    assert_eq!(0, q.len());

    // Put a value
    q.put(1);

    // Check the length again
    assert_eq!(1, q.len());

    // Remove the value
    assert_eq!(1, q.take());

    // Check the length
    assert_eq!(0, q.len());

    // Try taking on an empty queue
    assert!(q.poll().is_none());
}

#[test]
pub fn test_single_threaded_offer_timeout() {
    let q = LinkedQueue::with_capacity(1);

    q.offer(1).unwrap();

    let now = time::precise_time_ns();
    let result = q.offer_ms(2, 200);
    let delta = time::precise_time_ns() - now;
    assert!(result.is_err());
    assert!(delta >= 200_000_000, "actual={}", delta);
}

#[test]
pub fn test_single_threaded_poll_timeout() {
    let q = LinkedQueue::<u32>::new();

    let now = time::precise_time_ns();
    q.poll_ms(200);
    let delta = time::precise_time_ns() - now;
    assert!(delta >= 200_000_000, "actual={}", delta);
}

#[test]
pub fn test_single_consumer_single_producer() {
    let c = LinkedQueue::new();
    let p = c.clone();

    spawn(move || {
        sleep_ms(10);

        for i in 0..10_000 {
            p.put(i);
        }
    });

    for i in 0..10_000 {
        assert_eq!(i, c.take());
    }

    assert!(c.poll().is_none());
}

#[test]
pub fn test_single_consumer_multi_producer() {
    let c = LinkedQueue::new();

    for t in 0..10 {
        let p = c.clone();

        spawn(move || {
            sleep_ms(10);

            for i in 0..10_000 {
                p.put((t, i));
            }
        });
    }

    let mut vals = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    for _ in 0..10 * 10_000 {
        let (t, v) = c.take();
        assert_eq!(vals[t], v);
        vals[t] += 1;
    }
}

#[test]
pub fn test_multi_consumer_multi_producer() {
    let queue = LinkedQueue::new();
    let results = LinkedQueue::new();

    // Producers
    for t in 0..5 {
        let producer = queue.clone();

        spawn(move || {
            sleep_ms(10);

            for i in 1..1_000 {
                producer.put((t, i));
                thread::yield_now();
            }

            // Put an extra val to signal consumers to exit
            producer.put((t, 1_000_000));
        });
    }

    // Consumers
    for _ in 0..5 {
        let consumer = queue.clone();
        let results = results.clone();

        spawn(move || {
            let mut vals = vec![];
            let mut per_producer = [0, 0, 0, 0, 0];

            loop {
                let (t, v) = consumer.take();

                if v > 1_000 {
                    break;
                }

                assert!(per_producer[t] < v);
                per_producer[t] = v;

                vals.push((t, v));
                thread::yield_now();
            }

            results.put(vals);
        });
    }

    let mut all_vals = vec![];

    for _ in 0..5 {
        let vals = results.take();

        // TODO: Figure out this assertion
        // assert!(vals.len() >= 50 && vals.len() <= 3_000, "uneven batch size {}", vals.len());

        for &v in vals.iter() {
            all_vals.push(v);
        }
    }

    all_vals.sort();

    let mut per_producer = [1, 1, 1, 1, 1];

    for &(t, v) in all_vals.iter() {
        assert_eq!(per_producer[t], v);
        per_producer[t] += 1;
    }

    for &v in per_producer.iter() {
        assert_eq!(1_000, v);
    }
}

#[test]
pub fn test_queue_with_capacity() {
    let queue = LinkedQueue::with_capacity(8);

    for i in 0..8 {
        assert!(queue.offer(i).is_ok());
    }

    assert_eq!(Err(8), queue.offer(8));
    assert_eq!(Some(0), queue.poll());

    assert!(queue.offer(8).is_ok());

    for i in 1..9 {
        assert_eq!(Some(i), queue.poll());
    }
}

#[test]
pub fn test_multi_producer_at_capacity() {
    let queue = LinkedQueue::with_capacity(8);

    for _ in 0..8 {
        let queue = queue.clone();

        spawn(move || {
            for i in 0..1_000 {
                queue.put(i);
            }
        });
    }

    for _ in 0..8 * 1_000 {
        queue.take();
    }
}
