# Phase 50 — Baseline Performance Summary

The baseline performance was measured using a standalone Node.js application (limited to 0.5 CPU and 512MB RAM) without any Pulsur components.

## Test Configuration
- **Total Duration**: 5 minutes
- **Load Profile**: Ramping from 0 to 100 VUs
- **Workload**: Batch of 3 requests per VU (Health, Delay, Compute) ~300 Requests/sec peak attempt.
- **Resource Limits**: 0.5 CPU, 512MB RAM

## Key Metrics
| Metric | Result |
| :--- | :--- |
| **Total Requests** | 28,011 |
| **Success Rate (200 OK)** | 100% |
| **Throughput (Avg)** | 93.5 req/sec |
| **p(95) Response Time** | **2,919.39 ms** |
| **p(90) Response Time** | 2,340.49 ms |
| **Median Response Time** | 901.79 ms |
| **Error Rate** | 0.00% |

## Observations
- **High p(95) Latency**: The application showed significant latency degradation under 100 VUs, largely due to the CPU-intensive `/api/compute` endpoint competing for the 0.5 CPU limit.
- **No Rate Limiting**: As expected, the standalone Node.js app processed all incoming requests (even the slow ones) without any backpressure or rate limiting, leading to high response times for all users.
- **CPU Bottleneck**: The throughput (93.5 req/sec) was limited by the available CPU, causing a long queue of pending requests in the Node.js event loop.
