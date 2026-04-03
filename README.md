# pulsar 🦀

**pulsar** is a high-performance, modular infrastructure toolkit for building resilient distributed systems in Rust and Node.js.

## 🛸 Phase 11: Performance Absolute

Ferrum-core (Rust) sustains **22,505 Req/sec** under high load—**7.3x faster** than Node.js (3.0K) while maintaining <3ms p50 latency.

| Stack | Strategy | Req/sec | Latency (p50) | scale |
| :--- | :--- | :--- | :--- | :--- |
| **Node Baseline** | Event-Loop | 3,061 | 32 ms | 1x |
| **Ferrum (Rust)** | **Zero-Alloc** | **22,505** | **3 ms** | **7.3x 🚀** |

> see full [BENCHMARKS.md](./BENCHMARKS.md) for details.

## 🚀 Key Features

-   **High-Speed HTTP Server & Gateway**: Built atop Axum and Tower for maximum throughput.
-   **Intelligent Load Balancer**: Dynamic traffic distribution across backends.
-   **Distributed Rate Limiting**: Token bucket and sliding window implementations.
-   **Robust Distributed Queue**: Reliable job processing and synchronization.
-   **Resilience Patterns**: Circuit Breaker and Proxy out-of-the-box.
-   **First-Class Observability**: Integrated tracing, metrics, and monitoring dashboard.

## 📁 Repository Structure

```text
pulsar/
├── crates/             # Rust Crate Workspace
│   ├── http-server     # High-performance server
│   ├── gateway         # Entry point and request routing
│   ├── load-balancer   # Traffic distribution
│   ├── rate-limiter    # Policy enforcement
│   ├── queue           # Distributed job processing
│   ├── circuit-breaker # Fault tolerance
│   ├── proxy           # Traffic forwarding
│   └── observability   # Tracing and metrics
├── packages/           # Frontend & SDK Workspace
│   ├── js-sdk          # Node.js / Browser Client SDK
│   └── dashboard       # Next.js Management UI
└── .github/            # CI/CD Workflows
```

## 🛠️ Getting Started

### Prerequisites

-   **Rust**: `rustc` 1.75+ & `cargo`
-   **Node.js**: `node` 18+ & `npm`

### Setup

1.  **Clone the repository**:
    ```bash
    git clone https://github.com/pulsar/pulsar.git
    cd pulsar
    ```

2.  **Build the workspace**:
    ```bash
    cargo build --workspace
    ```

3.  **Install frontend dependencies**:
    ```bash
    npm install
    ```

## 📜 License

Distributed under the MIT License. See `LICENSE` for more information.

## 🤝 Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for details on our code of conduct and the process for submitting pull requests.

