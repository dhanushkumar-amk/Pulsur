# pulsur Benchmarks

Fresh local benchmark run captured on `2026-04-05` from the repo root on the current Windows workstation.

## Methodology

- Tool: `npx autocannon`
- Duration: `10s`
- Connections: `100`
- Threads: `4`
- Baseline: `node benchmarks/node_http.js`
- pulsur path: `target/release/examples/benchmark.exe`
- Warm-up before each run: `4s`
- Protocol under test: plain HTTP/1.1 on localhost

## Results

| Stack | Avg req/sec | p50 latency | p99 latency | Errors | Working set after warm-up | Startup to ready |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| Node.js baseline | 6,157.4 | 12 ms | 82 ms | 0 | 28.50 MB | 1982.15 ms |
| pulsur HTTP server | 8,114.9 | 9 ms | 63 ms | 0 | 7.49 MB | 1842.28 ms |

## Delta vs Baseline

- Throughput: `+31.8%`
- p50 latency: `-25.0%`
- p99 latency: `-23.2%`
- Working set: `-73.7%`
- Startup time: `-7.1%`

## Internal HTTP Microbenchmarks

Measured with `cargo test -p http_server --test benchmark -- --ignored --nocapture`.

| Operation | Result |
| :--- | ---: |
| `router.match_route` | `119,051 ops/sec` |
| `parse_request` | `28,338 ops/sec` |
| `send_response` | `14,687 ops/sec` |

## Assessment

For the current hello-world style workload, `pulsur` is clearly faster and lighter than the plain Node baseline. That is a good result.

This is not yet enough to claim final production-grade performance across real traffic shapes. The repo still needs longer soak tests, spike tests, multi-route gateway benchmarks, security audit coverage, and end-to-end measurements with the full stack enabled together.
