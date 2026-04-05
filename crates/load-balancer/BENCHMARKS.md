# Load Balancer Benchmarks

This document records the benchmark plan and result format for the `load-balancer` crate.

## Goals

- Compare `ferrum-lb` in front of 3 Node.js workers against a single Node.js worker.
- Compare `ferrum-lb` against the Node.js `http-proxy` package under the same traffic shape.
- Measure throughput, p99 latency, and load-balancer CPU usage.
- Measure failover speed when one backend dies during the benchmark run.

## Test Matrix

| Scenario | Topology | Metrics |
| :--- | :--- | :--- |
| Baseline | Single Node.js worker | req/sec, p50, p99 |
| Ferrum LB | `ferrum-lb` -> 3 Node.js workers | req/sec, p50, p99, LB CPU |
| http-proxy | `http-proxy` -> 3 Node.js workers | req/sec, p50, p99, proxy CPU |
| Failover | `ferrum-lb` -> 3 workers, kill 1 mid-run | redistribution time, error spike, p99 |

## Recommended Environment

- OS: Windows 11 or Linux
- Rust: stable toolchain used by this workspace
- Node.js: 18+
- Benchmark tool: `autocannon`
- Duration per run: 30s
- Concurrency: 100
- Threads: 4
- Warmup: 5s before sampling

## Suggested Commands

### Single Node.js worker

```powershell
node benchmarks/node_single.js
npx autocannon -c 100 -d 30 -t 4 http://127.0.0.1:4000
```

### Ferrum LB in front of 3 workers

```powershell
node benchmarks/node_worker.js 4001
node benchmarks/node_worker.js 4002
node benchmarks/node_worker.js 4003
cargo run -p load-balancer
npx autocannon -c 100 -d 30 -t 4 http://127.0.0.1:8080
```

### Node `http-proxy` comparison

```powershell
node benchmarks/http_proxy.js
npx autocannon -c 100 -d 30 -t 4 http://127.0.0.1:8081
```

### Failover run

1. Start the `ferrum-lb` topology.
2. Start `autocannon` for 30 seconds.
3. Kill one backend at the 10 second mark.
4. Record how long it takes for traffic to redistribute with no more requests hitting the dead worker.

## Results

Populate this table with measured numbers after running the scenarios above.

| Scenario | Req/sec | p50 | p99 | LB CPU | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Single Node.js | Pending | Pending | Pending | n/a | Baseline |
| Ferrum LB + 3 workers | Pending | Pending | Pending | Pending | |
| http-proxy + 3 workers | Pending | Pending | Pending | Pending | |

## Failover Result Template

| Event | Target |
| :--- | :--- |
| Backend killed at | 10s |
| Health check interval | 5s |
| Expected redistribution deadline | <= 10s |
| Observed redistribution time | Pending |
| Error burst during failover | Pending |

## Notes

- Record exact machine specs beside the final numbers for fair comparison.
- Keep the worker response payload identical across all scenarios.
- Capture CPU for the proxy or load balancer process only, not the combined worker fleet.
