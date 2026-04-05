---
title: Coming From Express
description: Side-by-side examples for teams moving from Express-style Node services.
---

# Coming From Express

If you already think in terms of `app.listen()`, middleware, and route handlers, Pulsur should feel familiar at the edges even though the fast path lives in Rust.

## Hello world

Express:

```js
const express = require("express");
const app = express();

app.get("/health", (_req, res) => {
  res.json({ ok: true });
});

app.listen(3000);
```

Pulsur SDK:

```js
const { createServer } = require("../../packages/js-sdk");

async function main() {
  const server = createServer();
  await server.listen(3000);
}

main();
```

## Rate limiting

Express with middleware packages often looks like this:

```js
app.use(rateLimit({ windowMs: 60_000, max: 100 }));
```

Pulsur today:

```js
const { createLimiter } = require("../../packages/js-sdk");

const limiter = createLimiter({ max: 100, window: "1m" });
await limiter.checkLimit("user:42");
```

In the gateway crate, the same concern moves into route-aware infrastructure config instead of app middleware.

## Queues and background work

A common Express shape:

```js
app.post("/emails", async (req, res) => {
  await jobs.add("send-email", req.body);
  res.status(202).json({ queued: true });
});
```

Pulsur queue helper:

```js
const { queue } = require("../../packages/js-sdk");

const emails = queue("emails");
await emails.enqueue({ to: "user@example.com" });
```

## Mental model shift

Express usually puts everything in-process with your app.

Pulsur moves toward:

- Rust for transport and concurrency-sensitive paths
- a JS control surface for integration
- infrastructure concerns like gateway transforms and queue durability living outside route files

## Migration strategy that hurts the least

1. Keep your app logic in Node first.
2. Put Pulsur in front of the app for the HTTP and gateway path.
3. Move high-churn middleware concerns like rate limiting into Pulsur config.
4. Adopt queue-backed background work where Node latency spikes hurt you most.

## What stays easy

- event-driven Node code
- JSON-first APIs
- local development without native bindings

## What changes

- packaging and release flow becomes more deliberate
- some behavior moves from code to config
- debugging crosses the Node and Rust boundary
