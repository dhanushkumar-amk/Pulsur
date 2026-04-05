---
title: ADR 0001 - Why Rust Over Go For The HTTP Server
description: Architecture decision record for the server implementation language.
---

# ADR 0001: Why Rust Over Go For The HTTP Server

## Status

Accepted

## Context

Pulsur is meant to sit on the hot path for HTTP traffic, retries, rate limits, and queue dispatch. The implementation language needed to handle:

- low-level networking control
- predictable performance under concurrency
- safe systems programming
- close integration with native Node packaging

## Decision

Use Rust for the core HTTP server and adjacent infrastructure components.

## Why

- ownership and borrowing reduce accidental shared-state bugs in systems code
- async Rust gives strong control over allocations and task boundaries
- the ecosystem supports safe TLS, HTTP primitives, and Node bindings
- shipping a native module or binary is a more natural extension of a Rust codebase than a JS-only stack

## Alternatives considered

### Go

Pros:

- simpler runtime model
- easier onboarding for many backend teams
- great standard library networking story

Cons for this project:

- less direct alignment with the repo's explicit memory-safety goals
- less appealing for a "Rust infrastructure for Node.js" positioning

### Node.js only

Pros:

- no native packaging friction
- fastest team iteration on surface APIs

Cons:

- gives up the main performance and systems-learning reason this project exists

## Consequences

- the repo gets stronger control over hot-path behavior
- releases become more complex because binaries and addons must be packaged
- the docs must explain the Rust and Node boundary clearly
