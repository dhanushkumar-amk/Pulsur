---
title: ADR 0002 - Use napi-rs For The Node Bridge
description: Architecture decision record for the Node.js FFI layer.
---

# ADR 0002: Use napi-rs For The Node Bridge

## Status

Accepted

## Context

Pulsur wants Node.js applications to adopt Rust components incrementally, not through a total rewrite. That requires a stable Node bridge with reasonable ergonomics.

## Decision

Use `napi-rs` for the Rust-to-Node bridge layer.

## Why

- it exposes Rust functions and structs through a stable Node-API surface
- it avoids handwritten C++ bindings
- it supports async methods naturally enough for the current server, queue, and rate limiter exports
- it keeps the Rust source expressive with `#[napi]` and `#[napi(object)]`

## Rejected alternatives

### FFI through a separate daemon only

Pros:

- avoids native addon packaging in app installs

Cons:

- adds operational overhead and protocol design work immediately
- makes local developer ergonomics worse

### Neon or handwritten Node addons

Pros:

- workable

Cons:

- more implementation overhead for this repo's goals
- less attractive when multiple Rust crates need a consistent bridge pattern

## Consequences

- package publishing must account for platform-specific native artifacts
- the JS SDK should keep fallback behavior for developer experience
- API design must stay narrow enough that the Rust and JS implementations do not drift
