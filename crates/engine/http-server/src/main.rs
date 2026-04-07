use http_server::{HttpServer, Router, Method, Response, ServerConfig};
use std::sync::Arc;
use futures::future::FutureExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let mut router = Router::new();
    
    // Add a basic health check
    router.add_http(Method::GET, "/health", Arc::new(|_req| {
        async move { Response::new(200) }.boxed()
    }));

    // Add a hello world
    router.add_http(Method::GET, "/", Arc::new(|_req| {
        async move { 
            let mut res = Response::new(200);
            res.body = b"Hello from Ferrum HTTP Server!".to_vec();
            res
        }.boxed()
    }));

    let config = ServerConfig::default();
    let server = HttpServer::new(router, config);

    println!("Starting server on http://127.0.0.1:8080");
    server.run("127.0.0.1:8080").await?;

    Ok(())
}
