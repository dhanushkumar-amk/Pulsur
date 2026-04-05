use axum::{response::IntoResponse, routing::get, Router};
use load_balancer::{Backend, BackendPool};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::info;

async fn health_handler() -> impl IntoResponse {
    "OK"
}

#[tokio::test]
async fn test_integration_health_check_kill_server() {
    let _ = tracing_subscriber::fmt::try_init();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let addr_str = addr.to_string();

    let app = Router::new().route("/health", get(health_handler));
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let pool = Arc::new(
        BackendPool::new()
            .with_health_check_interval(Duration::from_millis(200))
            .with_health_check_timeout(Duration::from_millis(100)),
    );
    pool.add(Backend::new(&addr_str, 1));

    pool.clone().spawn_health_checker();

    tokio::time::sleep(Duration::from_millis(100)).await;
    let status = pool.get_backends_status();
    assert!(status[0].healthy, "Backend should be healthy initially");
    assert!(
        pool.next_round_robin().is_some(),
        "Should route to the healthy backend"
    );

    info!("Killing backend server on {}...", addr_str);
    shutdown_tx.send(()).unwrap();
    let _ = server_handle.await;
    info!("Backend server killed.");

    let unhealthy_within_window = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = pool.get_backends_status();
            if !status[0].healthy {
                return status;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("backend should become unhealthy within 10 seconds");

    info!(
        "Backend status after kill: addr={}, healthy={}",
        unhealthy_within_window[0].address, unhealthy_within_window[0].healthy
    );
    assert!(
        !unhealthy_within_window[0].healthy,
        "Backend should be marked unhealthy after server kill"
    );
    assert!(
        pool.next_round_robin().is_none(),
        "Traffic should stop since no healthy backends"
    );
}
