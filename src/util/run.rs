use util::async::Future;

pub trait Run : Send {

    // /// Returns the `Run` instance associated with the current thread, None
    // /// otherwise
    // fn current<'a>() -> Option<&'a (Run+'static)> {
    //     unimplemented!();
    // }

    /// Runs the task on the underlying executor.
    fn run<F>(&self, task: F) where F: FnOnce() + Send;

    fn invoke<F, R>(&self, task: F) -> Future<R, ()> where F: FnOnce() -> R + Send, R: Send {
        let (complete, ret) = Future::pair();

        self.run(move || {
            complete.complete(task());
        });

        ret
    }
}
