//! # Futures & Streams
//!
//! Contains Future and Stream types as well as functions to operate on them.
//!
//! ## Future
//!
//! A future represents a value that will be provided sometime in the future.
//! The value may be computed concurrently in another thread or may be provided
//! upon completion of an asynchronous callback. The abstraction allows
//! describing computations to perform on the value once it is realized as well
//! as how to handle errors.
//!
//! One way to think of a Future is as a Result where the value is
//! asynchronously computed.
//!
//! # Stream

use syncbox::util::async::*;

/*
 * Last ported test: test_producer_fail_before_consumer_take
 */

// == Future tests ==
mod test_future_and;
mod test_future_await;
mod test_future_cancel;
mod test_future_or;
mod test_future_receive;

// == Join tests ==
mod test_join;

// == Select tests ==
mod test_select;

// == Sequence tests ==
mod test_sequence;

// == Stream tests ==
mod test_stream_await;
mod test_stream_cancel;
mod test_stream_iter;
mod test_stream_map;
mod test_stream_receive;
mod test_stream_reduce;
mod test_stream_collect;
mod test_stream_take;

/*
 *
 * ===== Helpers =====
 *
 */

fn nums(from: i32, to: i32) -> Stream<i32, ()> {
    Future::lazy(move || {
        debug!("range tick; from={}", from);

        if from < to {
            Ok(Some((from, nums(from + 1, to))))
        } else {
            Ok(None)
        }
    }).as_stream()
}
