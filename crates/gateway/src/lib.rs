use std::collections::HashMap;
use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use futures::future::BoxFuture;
use std::sync::Arc;

pub type GatewayRequest = Request<Body>;
pub type GatewayResponse = Response<Body>;

/// Context for the gateway request pipeline.
pub struct Context {
    pub request: GatewayRequest,
    pub metadata: HashMap<String, String>,
    pub upstream_url: Option<String>,
}

impl Context {
    pub fn new(request: GatewayRequest) -> Self {
        Self {
            request,
            metadata: HashMap::new(),
            upstream_url: None,
        }
    }
}

/// The plugin chain's "next" handle.
pub struct Next {
    pub(crate) plugins: Arc<Vec<Box<dyn Plugin>>>,
    pub(crate) index: usize,
}

impl Next {
    pub fn run<'a>(&'a self, ctx: &'a mut Context) -> BoxFuture<'a, GatewayResponse> {
        Box::pin(async move {
            if self.index < self.plugins.len() {
                let plugin = &self.plugins[self.index];
                let next = Next {
                    plugins: Arc::clone(&self.plugins),
                    index: self.index + 1,
                };
                plugin.call(ctx, next).await
            } else {
                // End of the pipeline reached without a terminal response.
                // Normally the last plugin (Passthrough) should handle this.
                StatusCode::NOT_FOUND.into_response()
            }
        })
    }
}

/// Trait for gateway plugins.
#[trait_variant::make(Send)] // if we want to avoid manual impl Send
pub trait Plugin: Send + Sync {
    async fn call(&self, ctx: &mut Context, next: Next) -> GatewayResponse;
}

/// The pipeline executor.
pub struct Pipeline {
    plugins: Arc<Vec<Box<dyn Plugin>>>,
}

impl Pipeline {
    pub fn new(plugins: Vec<Box<dyn Plugin>>) -> Self {
        Self {
            plugins: Arc::new(plugins),
        }
    }

    pub async fn execute(&self, mut ctx: Context) -> GatewayResponse {
        let next = Next {
            plugins: Arc::clone(&self.plugins),
            index: 0,
        };
        next.run(&mut ctx).await
    }
}

/// A simple passthrough plugin that forwards requests to an upstream server.
pub struct PassthroughPlugin {
    client: reqwest::Client,
}

impl Default for PassthroughPlugin {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Plugin for PassthroughPlugin {
    async fn call(&self, ctx: &mut Context, _next: Next) -> GatewayResponse {
        let upstream_url = ctx.upstream_url.as_ref().expect("Upstream URL must be set");
        
        // Construct the full URL
        // In a real scenario, you'd append path and query params.
        let path = ctx.request.uri().path();
        let query = ctx.request.uri().query().map(|q| format!("?{}", q)).unwrap_or_default();
        let target_url = format!("{}{}{}", upstream_url, path, query);

        // Convert axum request to reqwest request
        let mut req_builder = self.client.request(ctx.request.method().clone(), &target_url);
        
        // Copy headers
        for (name, value) in ctx.request.headers() {
            req_builder = req_builder.header(name, value);
        }

        // Copy body (this consumes the body)
        // Note: For streaming bodies, we'd need more complex handling.
        // For now, we take the body.
        let body = std::mem::replace(ctx.request.body_mut(), Body::empty());
        req_builder = req_builder.body(reqwest::Body::wrap_stream(body.into_data_stream()));

        match req_builder.send().await {
            Ok(resp) => {
                let mut builder = Response::builder().status(resp.status());
                
                // Copy response headers
                if let Some(headers_mut) = builder.headers_mut() {
                    for (name, value) in resp.headers() {
                        headers_mut.insert(name, value.clone());
                    }
                }

                builder.body(Body::from_stream(resp.bytes_stream())).unwrap()
            }
            Err(e) => {
                tracing::error!("Failed to forward request: {:?}", e);
                StatusCode::BAD_GATEWAY.into_response()
            }
        }
    }
}
