pub trait Run {
    /// Runs the task on the underlying executor.
    fn run<F>(&self, task: F) where F: FnOnce() + Send;
}
