---
title: Component Deep Dive
description: Design notes, tradeoffs, and internals for each current Pulsur component.
---

# Component Deep Dive

## HTTP Server

Responsibilities:

- parse HTTP/1.x requests
- route requests by method and path
- manage keep-alive, timeouts, and WebSocket upgrades
- expose a small napi-based `JsServer`

Tradeoffs:

- the Rust server path is fast and explicit
- the JS bridge currently exposes a simpler surface than the internal Rust server API

## Gateway

Key abstraction:

- `Plugin::call(&mut Context, Next) -> Response`

That gives the gateway a middleware pipeline similar to Koa, Axum layers, or Express, but implemented in Rust with explicit ownership and async futures.

Current plugin concerns:

- auth
- request and response transforms
- per-route rate limiting
- upstream retries and timeout behavior

Tradeoff:

- configuration is powerful, but the public JS helper is intentionally small today

## Load Balancer

The roadmap and crate layout point to a backend pool model with:

- round robin
- least connections
- weighted balancing
- active health state

This component matters most once traffic spreads across multiple Node workers or upstream services.

## Rate Limiter

Two major limiter shapes exist in the code:

- token bucket
- sliding window log

The JS SDK currently presents a friendlier limiter interface while the Rust crate carries the algorithmic detail and Redis-aware distributed mode.

Tradeoff:

- sliding windows are easy to reason about for route limits
- token buckets give smoother burst handling and predictable refill behavior

## Queue

The queue is one of the more operationally interesting components in the repo because it combines:

- in-memory queue state
- durable WAL replay
- compaction into snapshots
- WebSocket worker coordination

Tradeoff:

- durability adds write amplification
- durability also gives you restart recovery and better failure analysis

## Circuit Breaker

Purpose:

- stop hammering degraded upstreams
- fail fast once a backend is clearly unhealthy

It belongs close to gateway and proxy layers because that is where repeated upstream failure compounds latency.

## Reverse Proxy

Purpose:

- front requests
- route by policy
- optionally cache or normalize traffic

This is the layer where infrastructure concerns begin to look operational instead of application-specific.

## Observability

Current direction:

- `tracing`
- `metrics`
- dashboard package for a visual layer

Tradeoff:

- observability is easiest to underbuild early
- once the stack spans Rust and Node, it becomes one of the most important pieces for debugging
