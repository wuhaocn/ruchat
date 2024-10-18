use prometheus::{Counter, Histogram, HistogramOpts, opts};
use crate::metrics::MetricsRegistry;

#[derive(Clone)] // 添加此行
pub struct RequestMetrics {
    pub request_counter: Counter,
    pub request_duration: Histogram,
}

impl RequestMetrics {
    pub fn new(registry: &MetricsRegistry) -> Self {
        let request_counter = Counter::with_opts(opts!("request_count", "Total number of requests"))
            .expect("Creating request counter failed");

        let request_duration = Histogram::with_opts(HistogramOpts::new(
            "request_duration_seconds",
            "Request duration in seconds",
        ))
            .expect("Creating request duration histogram failed");

        registry.registry.register(Box::new(request_counter.clone())).unwrap();
        registry.registry.register(Box::new(request_duration.clone())).unwrap();

        Self {
            request_counter,
            request_duration,
        }
    }

    pub fn record_request(&mut self, duration: f64) {
        self.request_counter.inc();
        self.request_duration.observe(duration);
    }
}
