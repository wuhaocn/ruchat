use prometheus::{IntGauge, opts};
use sysinfo::{System};
use crate::metrics::MetricsRegistry;

#[derive(Clone)] // 添加此行
pub struct ResourceMetrics {
    pub cpu_usage: IntGauge,
    pub memory_usage: IntGauge,
    pub network_received: IntGauge,
    pub network_transmitted: IntGauge,
}

impl ResourceMetrics {
    pub fn new(registry: &MetricsRegistry) -> Self {
        let cpu_usage = IntGauge::with_opts(opts!("cpu_usage", "Current CPU usage"))
            .expect("Creating CPU usage gauge failed");
        let memory_usage = IntGauge::with_opts(opts!("memory_usage", "Current memory usage"))
            .expect("Creating memory usage gauge failed");
        let network_received = IntGauge::with_opts(opts!("network_received", "Total bytes received"))
            .expect("Creating network received gauge failed");
        let network_transmitted = IntGauge::with_opts(opts!("network_transmitted", "Total bytes transmitted"))
            .expect("Creating network transmitted gauge failed");

        registry.registry.register(Box::new(cpu_usage.clone())).unwrap();
        registry.registry.register(Box::new(memory_usage.clone())).unwrap();
        registry.registry.register(Box::new(network_received.clone())).unwrap();
        registry.registry.register(Box::new(network_transmitted.clone())).unwrap();

        Self {
            cpu_usage,
            memory_usage,
            network_received,
            network_transmitted,
        }
    }

    pub fn update_metrics(&self) {
        let sys = System::new_all();

        self.cpu_usage.set(sys.global_cpu_usage() as i64);
        self.memory_usage.set(sys.used_memory() as i64);
        self.network_received.set(0i64);
        self.network_transmitted.set(0i64);
    }
}
