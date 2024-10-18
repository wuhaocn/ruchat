## dep
```
[dependencies]
prometheus = { version = "0.13", features = [] }  # 或最新版本
warp = "0.3"         # 或最新版本
tokio = { version = "1", features = ["full"] }  # Tokio 作为异步运行时
```
## simple
```
use prometheus::{Encoder, IntCounter, Opts, Registry, TextEncoder};
use std::sync::{Arc, Mutex};
use warp::Filter;

#[tokio::main]
async fn main() {
    // 创建 Prometheus 注册表
    let registry = Arc::new(Mutex::new(Registry::new()));

    // 创建一个计数器
    let counter_opts = Opts::new("requests_total", "Total number of requests");
    let counter = IntCounter::with_opts(counter_opts).unwrap();

    // 将计数器注册到注册表中
    registry.lock().unwrap().register(Box::new(counter.clone())).unwrap();

    // 创建一个 warp 过滤器
    let registry_clone = Arc::clone(&registry);
    let routes = warp::path("metrics")
        .map(move || {
            let registry = registry_clone.lock().unwrap();
            let encoder = TextEncoder::new();
            let mut buffer = Vec::new();

            // 编码指标
            encoder.encode(&registry.gather(), &mut buffer).unwrap();
            // 设置响应
            warp::reply::with_header(buffer, "Content-Type", encoder.format_type())
        });

    // 创建一个处理请求的过滤器
    let request_counter = counter.clone();
    let hello_route = warp::path("hello")
        .map(move || {
            request_counter.inc(); // 增加计数器
            "Hello, world!"
        });

    // 合并路由
    let routes = hello_route.or(routes);
    println!("start ok");
    // 启动服务器
    warp::serve(routes)
        .run(([127, 0, 0, 1], 3030))
        .await;
}

```

## view

http://127.0.0.1:3030/hello
http://127.0.0.1:3030/metrics