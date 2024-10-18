use prometheus::{IntCounter, Opts, Registry};
use std::sync::{Arc, Mutex};

pub struct TokioMetrics {
    pub registry: Arc<Mutex<Registry>>,
    pub task_counter: IntCounter,
}

impl TokioMetrics {
    pub fn new() -> Self {
        let registry = Arc::new(Mutex::new(Registry::new()));

        let task_opts = Opts::new("task_counter", "Number of tasks executed");
        let task_counter = IntCounter::with_opts(task_opts).unwrap();
        registry.lock().unwrap().register(Box::new(task_counter.clone())).unwrap();

        TokioMetrics {
            registry,
            task_counter,
        }
    }

    pub fn increment_task_counter(&self) {
        self.task_counter.inc();
    }
}
