---
title: JS SDK API Reference
description: API reference for the current JS bridge in packages/js-sdk.
---

# JS SDK API Reference

This page documents the API currently exposed by `packages/js-sdk/index.js` and `packages/js-sdk/index.d.ts`.

## Entry points

```js
const {
  Pulsur,
  createServer,
  createLimiter,
  gateway,
  queue,
  rateLimit,
  Queue,
  Worker,
  PulsurServer,
  PulsurLimiter,
  PulsurError,
  PulsurConnectionError,
  PulsurTimeoutError,
  PulsurRateLimitError,
  PulsurNativeBindingError,
} = require("../../packages/js-sdk");
```

## `createServer(): PulsurServer`

Creates a Node-facing HTTP server bridge.

Behavior:

- uses the native Rust binding if `http_server.node` is available
- falls back to a lightweight Node `http.createServer()` implementation otherwise

### `PulsurServer.listen(port: number): Promise<this>`

Starts listening on `127.0.0.1`.

Events:

- `connect`
- `error`

### `PulsurServer.close(): Promise<void>`

Stops the native or fallback server.

Events:

- `disconnect`
- `error`

### `PulsurServer.port: number | null`

The currently bound port after startup.

## `createLimiter(options): PulsurLimiter`

Creates an in-process limiter.

```ts
type Options = {
  max: number;
  window: string | number;
};
```

Accepted `window` values:

- number of milliseconds
- strings like `500ms`, `10s`, `1m`

### `PulsurLimiter.checkLimit(key: string): Promise<RateLimitResult>`

Returns:

```ts
type RateLimitResult = {
  allowed: boolean;
  limit: number;
  remaining: number;
  retry_after_secs: number;
  reset_after_secs: number;
};
```

Throws `PulsurRateLimitError` when the request is rejected.

## `queue(name, options): Queue`

Creates a queue client.

```ts
type QueueOptions = {
  url?: string;
  backoffBaseMs?: number;
  backoffMaxMs?: number;
};
```

Behavior modes:

- local in-memory fallback if no native queue or remote `url` is configured
- native queue bridge when the queue addon is present
- remote WebSocket queue when `url` is set

### `Queue.enqueue(payload, maxAttempts?): Promise<QueueJob>`

Adds a new pending job.

### `Queue.schedule(payload, runAt, maxAttempts?): Promise<QueueJob>`

Creates a scheduled job.

`runAt` may be a `Date` or ISO string.

### `Queue.dequeue(): Promise<QueueJob | null>`

Returns the next pending job or `null`.

### `Queue.ack(id): Promise<QueueJob>`

Marks a processing job as completed.

### `Queue.nack(id, reason): Promise<QueueJob>`

Marks a processing job as failed. In fallback mode this either retries or moves the job to dead letter once `max_attempts` is exhausted.

### `Queue.stats(): Promise<QueueStats>`

Returns:

```ts
type QueueStats = {
  pending: number;
  processing: number;
  scheduled: number;
  completed: number;
  dead_letter: number;
};
```

### `Queue.process(handler, options): Worker`

Creates a polling worker loop.

```ts
type WorkerOptions = {
  pollIntervalMs?: number;
};
```

The handler receives:

- parsed payload
- raw queue job metadata

### `Worker.stop(): Promise<void>`

Stops the poll loop cleanly.

## `gateway(config)`

Current shape:

```ts
gateway({
  upstream: "http://localhost:4000",
  plugins: [],
});
```

The JS helper is intentionally thin right now. The richer gateway behavior lives in the Rust crate and the YAML config loader.

## `rateLimit(options)`

Alias for `createLimiter(options)`.

## Errors

### `PulsurError`

Base error class with:

- `message`
- `code`
- `cause`

### `PulsurConnectionError`

Used for connection and socket failures.

Code:

```text
PULSUR_CONNECTION_ERROR
```

### `PulsurTimeoutError`

Used for timeout-like failures.

Code:

```text
PULSUR_TIMEOUT_ERROR
```

### `PulsurRateLimitError`

Thrown when a limiter rejects a request.

Code:

```text
PULSUR_RATE_LIMITED
```

Additional property:

- `details`

### `PulsurNativeBindingError`

Used when native module loading fails.

Code:

```text
PULSUR_NATIVE_BINDING_ERROR
```

## Verified examples

- `node ./docs/examples/getting-started.js`
- `node ./docs/examples/rate-limiter.js`
- `node ./docs/examples/queue-worker.js`
