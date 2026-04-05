---
title: Troubleshooting
description: Common local issues and how to get unstuck quickly.
---

# Troubleshooting

## `Unsupported platform for @pulsur/http-server-*`

Cause:

- platform packages are intentionally OS-specific

Fix:

- install the main package rather than adding every platform package as an active workspace
- for local repo work, use `node ./scripts/stage-npm-binary.js --component http-server --profile debug`

## Native binding is missing and the SDK fell back to Node

How to tell:

- the server still works
- behavior is simpler than the Rust path

Fix:

```bash
cargo build --workspace
```

Then point the SDK at the generated bindings if needed using:

- `PULSUR_HTTP_SERVER_BRIDGE`
- `PULSUR_RATE_LIMITER_BRIDGE`
- `PULSUR_QUEUE_BRIDGE`

## `PulsurRateLimitError`

Cause:

- the limiter rejected the request

Handle it explicitly:

```js
try {
  await limiter.checkLimit("client-a");
} catch (error) {
  if (error.code === "PULSUR_RATE_LIMITED") {
    console.error(error.details);
  }
}
```

## Queue says a job is not processing

Cause:

- `ack()` or `nack()` was called for a job that was never dequeued or was already finished

Fix:

- confirm your worker only acknowledges jobs returned by `dequeue()`
- avoid double-acking in error paths

## Docs site does not build

Try:

```bash
cd docs
npm install
npm run build
```

If the build still fails, clear cached state:

```bash
npm run clear
```

## Example verification

Run all verified examples:

```bash
cd docs
npm run verify:examples
```
