---
title: FFI Bridge
description: How Node.js talks to Rust in the current Pulsur repo.
---

# FFI Bridge

Pulsur uses `napi-rs` to expose Rust functionality to Node.js.

## The shape of the bridge

At a high level:

```mermaid
flowchart LR
  JS[packages/js-sdk] --> NativeLoader[native.js loader]
  NativeLoader --> NapiModule[*.node addon]
  NapiModule --> RustAPI[#[napi] Rust exports]
  RustAPI --> Runtime[Tokio and Rust data structures]
```

## Current exports

### HTTP server bridge

In `crates/http-server/src/lib.rs` the bridge exports:

- `JsServer`
- `create_server()`

`JsServer` wraps:

- a mutex-protected state object
- a shutdown channel
- a Tokio task handle
- the active port

### Rate limiter bridge

In `crates/rate-limiter/src/lib.rs` the bridge exports:

- `JsSlidingWindowLimiter`
- `create_limiter(max_requests, window_ms)`

This exposes async `check_limit()` directly into Node and maps Rust structs into `#[napi(object)]` result objects.

### Queue bridge

In `crates/queue/src/lib.rs` the bridge exports:

- `JsQueue`
- `JsWorker`
- `create_queue()`

This lets Node call enqueue, dequeue, ack, nack, stats, and worker polling methods without manually speaking the queue protocol.

## JS side behavior

The JS SDK does not assume the native modules always exist.

Instead it:

- tries to load native bindings
- falls back to Node implementations when appropriate
- normalizes errors into typed JS subclasses
- emits `EventEmitter` lifecycle events

That fallback behavior is why local development still works before full native packaging is finished.

## Why napi-rs here

Advantages:

- stable Node integration model
- fewer handwritten C++ bindings
- direct mapping from Rust structs to JS-visible objects
- good ergonomics for async exports

Tradeoff:

- packaging native binaries across platforms becomes part of your product surface

## Boundaries to keep clear

Good candidates for the bridge:

- compute-heavy or concurrency-sensitive operations
- stable interfaces like server lifecycle or limit checks
- data structures where Rust ownership helps correctness

Bad candidates:

- very high-churn product logic
- APIs that change weekly
- interfaces where JS fallback semantics differ wildly from native behavior
