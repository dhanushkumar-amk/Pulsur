use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing to see logs
    tracing_subscriber::fmt::init();

    info!("🚀 Welcome to Phase 2 - Tokio Deep Dive!");

    // Start parts in background or run specific one
    // For simplicity, we'll implement and call our demo functions here.

    // 1. TCP Echo Server Demo (handled in separate task)
    let echo_handle = tokio::spawn(async {
        if let Err(e) = run_echo_server("127.0.0.1:8080").await {
            error!("Echo server error: {:?}", e);
        }
    });

    // 2. Heartbeat Demo (every 5 seconds)
    let heartbeat_handle = tokio::spawn(async {
        run_heartbeat(Duration::from_secs(5)).await;
    });

    // 3. Select & Timeout Demo
    let timeout_demo = tokio::spawn(async {
        if let Err(e) = run_timeout_select_server("127.0.0.1:8081").await {
            error!("Timeout/Select server error: {:?}", e);
        }
    });

    // 4. MPSC Channel Demo
    let mpsc_handle = tokio::spawn(async {
        run_mpsc_demo().await;
    });

    // Join all or just wait (in a real app you'd handle signals)
    // For now, let's keep running for a bit
    tokio::select! {
        _ = echo_handle => info!("Echo server closed"),
        _ = heartbeat_handle => info!("Heartbeat stopped"),
        _ = timeout_demo => info!("Timeout demo stopped"),
        _ = mpsc_handle => info!("MPSC demo finished"),
    }

    Ok(())
}

/// --- Part 1 & 2: TCP Echo Server with tokio::spawn ---
/// Build multi-threaded TCP echo server using tokio::net::TcpListener
/// Use tokio::spawn to handle each connection on a separate task
async fn run_echo_server(addr: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("✅ Echo Server listening on {}", addr);

    loop {
        let (mut socket, peer_addr) = listener.accept().await?;
        info!("🤝 New connection from: {}", peer_addr);

        tokio::spawn(async move {
            let mut buf = [0; 1024];

            loop {
                let n = match socket.read(&mut buf).await {
                    // socket closed
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(e) => {
                        error!("failed to read from socket; err = {:?}", e);
                        return;
                    }
                };

                if let Err(e) = socket.write_all(&buf[0..n]).await {
                    error!("failed to write to socket; err = {:?}", e);
                    return;
                }
            }
        });
    }
}

/// --- Part 3: tokio::select! & Timeout ---
/// Learn tokio::select! — write a server that races a timeout against a read
async fn run_timeout_select_server(addr: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("⏱️ Timeout/Select Server listening on {}", addr);

    loop {
        let (mut socket, _addr) = listener.accept().await?;

        tokio::spawn(async move {
            let mut buf = [0; 1024];

            loop {
                // Race a 10-second timeout against waiting for data
                tokio::select! {
                    result = socket.read(&mut buf) => {
                        match result {
                            Ok(0) => break, // closed
                            Ok(n) => {
                                if let Err(e) = socket.write_all(&buf[0..n]).await {
                                    error!("Write error: {:?}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Read error: {:?}", e);
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {
                        info!("⌛ Connection timed out due to inactivity");
                        let _ = socket.write_all(b"TIMEOUT\n").await;
                        break;
                    }
                }
            }
        });
    }
}

/// --- Part 4: Heartbeat with tokio::time::interval ---
/// Learn tokio::time::interval — write a heartbeat that fires every 5 seconds
async fn run_heartbeat(dur: Duration) {
    let mut ticker = interval(dur);
    info!("💓 Heartbeat started every {:?}", dur);

    loop {
        ticker.tick().await;
        info!("💓 Ping! Pulsur System is Alive.");
    }
}

/// --- Part 5: tokio::sync::mpsc ---
/// Learn tokio::sync::mpsc — producer/consumer channel between tasks
async fn run_mpsc_demo() {
    let (tx, mut rx) = mpsc::channel(32);

    tokio::spawn(async move {
        for i in 1..=5 {
            info!("📤 Producer: Sending message {}", i);
            let _ = tx.send(format!("MSG-{}", i)).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    while let Some(msg) = rx.recv().await {
        info!("📥 Consumer: Received: {}", msg);
    }

    info!("🏁 MPSC Demo Complete");
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    /// Write test: spawn 100 tasks, each sends a message, verify all received
    #[tokio::test]
    async fn test_100_tasks_mpsc() {
        let (tx, mut rx) = mpsc::channel(100);

        for i in 0..100 {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                let _ = tx_clone.send(i).await;
            });
        }

        // Drop original tx so rx knows when to stop if it wants to iterate
        drop(tx);

        let mut received = Vec::new();
        while let Some(i) = rx.recv().await {
            received.push(i);
        }

        assert_eq!(received.len(), 100);
        for i in 0..100 {
            assert!(received.contains(&i));
        }
    }
}
