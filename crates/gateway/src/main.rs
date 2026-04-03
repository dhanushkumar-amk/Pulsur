use gateway::{Context, Pipeline, TransformPlugin, PassthroughPlugin};
use http_server::{HttpServer, Router, Method, ServerConfig};
use std::sync::Arc;
use futures::future::FutureExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // 1. Configure the Transform Plugin from YAML
    let yaml_config = r#"
prefix_strip: "/api/v1"
inject_response_headers:
  Access-Control-Allow-Origin: "*"
  X-Gateway-Version: "Phase-14"
strip_headers:
  - "X-Powered-By"
  - "Server"
body_transformations:
  "user_id": "id"
  "status_code": "code"
"#;
    let transform_plugin = TransformPlugin::from_config(yaml_config)?;

    // 2. Build the Plugin Pipeline
    let pipeline = Arc::new(Pipeline::new(vec![
        Box::new(transform_plugin),
        Box::new(PassthroughPlugin::default()), // Upstream forwarder
    ]));

    // 3. Create a Router to register our Gateway handler
    let mut router = Router::new();
    
    // Catch-all route to handle everything through the gateway pipeline
    // We can't really do "catch all" easily in your current router yet (due to split logic)
    // but we can register the prefix we're about to strip.
    let pipeline_get = Arc::clone(&pipeline);
    router.add_http(Method::GET, "/api/v1/:resource", Arc::new(move |req| {
        let pipeline = Arc::clone(&pipeline_get);
        async move {
            let mut ctx = Context::new(req);
            ctx.upstream_url = Some("http://127.0.0.1:4000".to_string());
            pipeline.execute(ctx).await
        }.boxed()
    }));

    let pipeline_post = Arc::clone(&pipeline);
    router.add_http(Method::POST, "/api/v1/:resource", Arc::new(move |req| {
        let pipeline = Arc::clone(&pipeline_post);
        async move {
            let mut ctx = Context::new(req);
            ctx.upstream_url = Some("http://127.0.0.1:4000".to_string());
            pipeline.execute(ctx).await
        }.boxed()
    }));

    // 4. Start the server
    let config = ServerConfig::default();
    let server = HttpServer::new(router, config);

    println!("Gateway [Phase 14] starting on http://127.0.0.1:3000 -> Forwarding to http://127.0.0.1:4000");
    server.run("127.0.0.1:3000").await?;

    Ok(())
}
