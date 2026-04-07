---
sidebar_position: 1
title: Pulsur Documentation
description: Start here for setup, API reference, migration notes, and architecture internals.
---

# Pulsur Documentation

Pulsur is a Rust-powered infrastructure toolkit aimed at Node.js applications that need more control over performance-sensitive paths like HTTP serving, gateway behavior, rate limiting, and queue-backed work.

This docs site is organized around two goals:

- get a working local integration fast
- explain how the current repository is put together so you can extend it safely

## What exists in this repo today

- A Rust workspace with crates for HTTP serving, gateway logic, load balancing, rate limiting, queueing, proxying, circuit breaking, and observability
- A Node-facing JS SDK published as `@pulsur/js-sdk` that wraps native bindings when present and falls back to pure Node implementations when they are not
- npm packaging for the published HTTP server launcher in `packages/@pulsur/http-server`
- Release automation for versioning and packaging via GitHub Actions

## Recommended reading order

1. [Getting Started](./getting-started.md)
2. [JS SDK API Reference](./api/js-sdk.md)
3. [Coming From Express](./guides/coming-from-express.md)
4. [Architecture Overview](./architecture/overview.md)

## Reality check

These docs track the current codebase, not the aspirational full roadmap. Where a component is still early or has a fallback mode, the page calls that out directly.
