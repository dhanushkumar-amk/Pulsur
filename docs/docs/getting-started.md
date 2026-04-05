---
sidebar_position: 2
title: Getting Started In 5 Minutes
description: Install Pulsur locally, run the JS bridge, and verify a real response.
---

# Getting Started In 5 Minutes

This guide uses the current local repo state and the JS SDK bridge. It gives you a working server, a rate limiter, and a queue example without requiring a full cross-platform publish first.

## Prerequisites

- Node.js 18+
- npm 10+
- Rust stable toolchain

## 1. Install dependencies

From the repository root:

```bash
npm install
cargo build --workspace
```

## 2. Start with the simplest HTTP server

Runnable example:

```bash
node ./docs/examples/getting-started.js
```

What it does:

- creates a Pulsur server through `packages/js-sdk`
- listens on a local port
- fetches `/health`
- prints the JSON response
- closes cleanly

Expected output looks like this:

```text
Pulsur server listening on 38765
GET /health -> 200 {"ok":true}
```

## 3. Try the rate limiter

Runnable example:

```bash
node ./docs/examples/rate-limiter.js
```

This shows:

- allowed requests inside a window
- a typed `PulsurRateLimitError`
- the `code` property you can branch on in application code

## 4. Try the queue worker

Runnable example:

```bash
node ./docs/examples/queue-worker.js
```

This exercises:

- `queue()`
- `enqueue()`
- `process()`
- `Worker.stop()`

## 5. Use the packaged HTTP server binary

If you want the published launcher package flow instead of the SDK bridge:

```bash
node ./scripts/stage-npm-binary.js --component http-server --profile debug
npm pack ./packages/@pulsur/http-server
```

Then install the tarball into a clean app and resolve the executable:

```js
const { getBinaryPath } = require("@pulsur/http-server");
console.log(getBinaryPath());
```

## Minimal hello world

```js
const { createServer } = require("../../packages/js-sdk");

async function main() {
  const server = createServer();
  await server.listen(3000);
  console.log(`Listening on ${server.port}`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
```

## What to read next

- [JS SDK API Reference](./api/js-sdk.md)
- [Troubleshooting](./guides/troubleshooting.md)
- [Architecture Overview](./architecture/overview.md)
