use prometheus::{Encoder, TextEncoder};
use std::net::{SocketAddr};
use tokio::time::{sleep, Duration};
use warp::Filter;

use ru_metrics_starter::{MetricsRegistry, ResourceMetrics, RequestMetrics};

#[tokio::main]
async fn main() {
    let metrics_registry = MetricsRegistry::new();

    // Initialize resource and request metrics
    let resource_metrics = ResourceMetrics::new(&metrics_registry);
    let request_metrics = RequestMetrics::new(&metrics_registry); // 不需要声明为可变，因为我们只是在这里创建一个实例

    // Setup a route to expose the metrics
    let metrics_route = warp::path("metrics").map({
        let registry = metrics_registry.registry.clone();
        move || {
            let encoder = TextEncoder::new();
            let metric_families = registry.gather();
            let mut buffer = Vec::new();
            encoder.encode(&metric_families, &mut buffer).expect("Encoding failed");
            String::from_utf8(buffer).expect("Invalid UTF-8")
        }
    });

    // Start a background task to update resource metrics every 5 seconds
    let resource_metrics_clone = resource_metrics.clone();
    tokio::spawn(async move {
        loop {
            resource_metrics_clone.update_metrics();
            sleep(Duration::from_secs(5)).await;
        }
    });

    // Simulate handling requests
    let request_route = warp::path("request").and_then({
        let request_metrics = request_metrics.clone(); // 克隆 request_metrics 以便在异步上下文中使用
        move || {
            let mut request_metrics = request_metrics.clone(); // 克隆 request_metrics
            async move {
                let start = std::time::Instant::now();
                // Simulate request processing logic
                sleep(Duration::from_millis(100)).await; // 模拟请求延迟
                let duration = start.elapsed().as_secs_f64();
                request_metrics.record_request(duration); // 记录请求持续时间
                Ok::<_, warp::Rejection>(format!("Handled request in {:.2} seconds", duration))
            }
        }
    });

    // Run the server
    let addr: SocketAddr = "127.0.0.1:3030".parse().unwrap();
    warp::serve(metrics_route.or(request_route)).run(addr).await;
}
