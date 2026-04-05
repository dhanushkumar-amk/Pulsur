---
title: Architecture Overview
description: High-level request flow, data flow, and deployment shape for Pulsur.
---

# Architecture Overview

Pulsur is structured as a Rust workspace with a thin Node-facing bridge. The current repository favors one shared versioned workspace, repo-local examples, and JS integration points that remain usable even when a native binding is unavailable.

## Request flow

```mermaid
flowchart LR
  Client[Client] --> HTTP[HTTP Server crate]
  HTTP --> Gateway[Gateway pipeline]
  Gateway --> RL[Rate limiter]
  Gateway --> LB[Load balancer]
  LB --> App[Node.js app]
  Gateway --> Obs[Observability]
```

## Queue and worker flow

```mermaid
flowchart LR
  Producer[Producer] --> QueueAPI[Queue API]
  QueueAPI --> WAL[Write-ahead log]
  WAL --> Snapshot[Snapshot.bin]
  QueueAPI --> Pending[Pending jobs]
  Pending --> Worker[Worker or WebSocket consumer]
  Worker --> Ack[Ack or Nack]
  Ack --> WAL
```

## Deployment topology

```mermaid
flowchart TB
  subgraph Edge
    HTTP[HTTP Server]
    Gateway[Gateway]
  end

  subgraph Core
    LB[Load Balancer]
    RL[Rate Limiter]
    Queue[Queue]
    Proxy[Reverse Proxy]
    CB[Circuit Breaker]
    Obs[Observability]
  end

  subgraph Apps
    App1[Node worker 1]
    App2[Node worker 2]
    App3[Node worker 3]
  end

  HTTP --> Gateway --> LB
  LB --> App1
  LB --> App2
  LB --> App3
  Gateway --> RL
  Gateway --> CB
  Queue --> Obs
  Gateway --> Obs
```

## Design themes in the current codebase

- shared versioning through the Rust workspace
- explicit packaging for npm-distributed binaries
- JS fallbacks that keep local development moving
- persistence via append-only WAL plus snapshot compaction in the queue
- plugin-style request processing in the gateway

## What is production-ready vs early-stage

More mature in repo terms:

- HTTP parser and server core
- gateway pipeline and config parsing
- queue persistence model
- rate limiter algorithms
- release and npm packaging flow

Still evolving:

- breadth of published npm packages beyond the HTTP server launcher
- dashboard and broader docs polish
- some cross-platform native packaging paths that depend on CI runners
