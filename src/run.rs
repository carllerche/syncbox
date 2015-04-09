
pub trait Run<T: Task> {
    /// Runs the task on the underlying executor.
    fn run(&self, task: T);
}

/// A value that can run a unit of work.
pub trait Task : Send {
    /// Run the unit of work
    fn run(self);
}

impl<F: FnOnce() + Send> Task for F {
    fn run(self) {
        self()
    }
}
