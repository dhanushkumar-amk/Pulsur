use clap::{Parser, Subcommand};
use std::sync::Arc;
use tokio::net::TcpListener;
use axum::{
    body::Body,
    extract::{State, WebSocketUpgrade},
    http::{Request, Response, StatusCode, HeaderMap},
    response::IntoResponse,
    routing::{any, get},
    Router,
};
use gateway::HotReloadGateway;
use load_balancer::{Backend, BackendPool};
use queue::Queue;
use futures_util::StreamExt;

#[derive(Parser)]
#[command(name = "pulsur-test")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Gateway
    Gateway {
        #[arg(short, long)]
        config: String,
    },
    /// Start a Load Balancer
    Lb {
        #[arg(short, long, default_value = "0.0.0.0:8081")]
        addr: String,
        #[arg(short, long)]
        backends: Vec<String>,
    },
    /// Start a Queue Service
    Queue {
        #[arg(short, long, default_value = "0.0.0.0:8082")]
        addr: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Gateway { config } => {
            println!("Starting Gateway with config: {}", config);
            HotReloadGateway::start(&config).await?;
        }
        Commands::Lb { addr, backends } => {
            println!("Starting Load Balancer on {} with backends: {:?}", addr, backends);
            let pool = Arc::new(BackendPool::new());
            for b in backends {
                pool.add(Backend::new(&b, 1));
            }
            pool.clone().spawn_health_checker();

            let app = Router::new()
                .route("/", any(lb_proxy_handler))
                .route("/*path", any(lb_proxy_handler))
                .with_state(pool);

            let listener = TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }
        Commands::Queue { addr } => {
            println!("Starting Queue Service on {}", addr);
            let queue = Arc::new(Queue::new());
            
            let app = Router::new()
                .route("/ws", get(queue_ws_handler))
                .with_state(queue);

            let listener = TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}

async fn lb_proxy_handler(
    State(pool): State<Arc<BackendPool>>,
    req: Request<Body>,
) -> Response<Body> {
    let backend = match pool.next_round_robin() {
        Some(b) => b,
        None => return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(Body::from("No healthy backends"))
            .unwrap(),
    };

    let client = reqwest::Client::new();
    let method = req.method().clone();
    let path = req.uri().path_and_query().map(|v| v.as_str()).unwrap_or("");
    let target_url = format!("http://{}{}", backend.address, path);

    let mut builder = client.request(method, target_url);
    for (key, value) in req.headers() {
        builder = builder.header(key, value);
    }
    
    match builder.send().await {
        Ok(res) => {
            let status = res.status();
            let mut headers = HeaderMap::new();
            for (key, value) in res.headers() {
                headers.insert(key.clone(), value.clone());
            }
            let bytes = res.bytes().await.unwrap_or_default();
            let mut response = Response::new(Body::from(bytes));
            *response.status_mut() = status;
            *response.headers_mut() = headers;
            response
        }
        Err(_) => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::from("Upstream error"))
            .unwrap(),
    }
}

async fn queue_ws_handler(
    ws: WebSocketUpgrade,
    State(_queue): State<Arc<Queue>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        while let Some(msg) = socket.recv().await {
            if let Ok(axum::extract::ws::Message::Text(text)) = msg {
                socket.send(axum::extract::ws::Message::Text(format!("Ack: {}", text))).await.ok();
            }
        }
    })
}
