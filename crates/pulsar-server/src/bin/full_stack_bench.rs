use http_server::{HttpServer, Method, Response, Router, ServerConfig};
use load_balancer::{Backend, BackendPool, BackendPoolConfig};
use rate_limiter::TokenBucketRateLimiter;
use std::sync::Arc;
use futures::future::FutureExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // 1. Setup Backend (Pulsar Server)
    let mut backend_router = Router::new();
    backend_router.add_http(Method::GET, "/", Arc::new(|_| {
        async move {
            let mut res = Response::new(200);
            res.body = "ok".into();
            res
        }.boxed()
    }));
    
    let backend_server = HttpServer::new(backend_router, ServerConfig::default());
    tokio::spawn(async move {
        // Run on 8081
        let _ = backend_server.run("127.0.0.1:8081").await;
    });

    // 2. Setup LB
    let pool = BackendPool::from_config(BackendPoolConfig::default());
    pool.add(Backend::new("127.0.0.1:8081", 1));
    let pool_arc = Arc::new(pool);

    // 3. Setup Rate Limiter (In-memory)
    let rl = TokenBucketRateLimiter::new(1000000.0, 1000000.0)?;
    let rl_arc = Arc::new(rl);

    // 4. Setup Gateway Router
    let mut gateway_router = Router::new();
    gateway_router.add_http(Method::GET, "/", Arc::new(move |_| {
        let pool = pool_arc.clone();
        let rl = rl_arc.clone();
        async move {
            // Rate limit check
            if !rl.check_key("global", 1.0).unwrap().allowed {
                let mut res = Response::new(429);
                res.body = "Too Many Requests".into();
                return res;
            }
            
            // LB Proxy
            let _backend = pool.next_round_robin().expect("expected backend");
            
            // Simulated Proxy response
            let mut res = Response::new(200);
            res.body = "Proxied through Pulsur Stack".into();
            res
        }.boxed()
    }));

    let server = HttpServer::new(gateway_router, ServerConfig {
        max_conns: 1000,
        ..ServerConfig::default()
    });
    
    println!("🛰️ Pulsur Full Stack Baseline listening on 127.0.0.1:8080");
    server.run("127.0.0.1:8080").await?;

    Ok(())
}
