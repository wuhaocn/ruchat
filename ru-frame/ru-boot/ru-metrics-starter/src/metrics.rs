use prometheus::Registry;

pub struct MetricsRegistry {
    pub registry: Registry,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        let registry = Registry::new();
        Self { registry }
    }
}
