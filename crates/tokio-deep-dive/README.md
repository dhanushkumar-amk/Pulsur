# Phase 2: Tokio Async Runtime Deep Dive

This crate implements the core concepts of the Tokio async runtime as requested in Phase 2.

## Features Implemented

1.  **Multi-threaded TCP Echo Server**: Uses `TcpListener` and `tokio::spawn` to handle each client in a separate asynchronous task.
2.  **Timeout Racing (`tokio::select!`)**: A second server listening on port `8081` that closes connections if no data is received for 10 seconds.
3.  **Heartbeat System**: Uses `tokio::time::interval` to log a "Ping" every 5 seconds.
4.  **Task Communication (`tokio::sync::mpsc`)**: A producer/consumer demo showing how to send data between concurrent tasks using channels.
5.  **Stress Testing**: A unit test that spawns 100 tasks to verify message integrity across high concurrency.

## How to Run

### 1. Run the Demos
Run the main application to see the Heartbeat, MPSC demo, and both servers start:
```bash
cargo run -p tokio-deep-dive
```

### 2. Test the Echo Server
Connect via `telnet` or `nc`:
```bash
# Terminal 1
nc 127.0.0.1 8080
```

### 3. Test the Timeout Server
Connect and wait for 10 seconds:
```bash
# Terminal 2
nc 127.0.0.1 8081
```

### 4. Run the Benchmarking Test
Run the high-concurrency test:
```bash
cargo test -p tokio-deep-dive
```

### 5. Measurement (wrk)
Note: `wrk` is primarily a Linux tool. If you are on Windows, you can run this via WSL:
```bash
wrk -t12 -c1000 -d30s http://127.0.0.1:8080
```
*(Note: Since this is a TCP Echo server and not HTTP, standard `wrk` might need a script or a dedicated TCP benchmarker like `flame-graph` or `iperf` for deep analysis, but the architecture is ready to handle 1000+ tasks easily).*
