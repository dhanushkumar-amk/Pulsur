"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");

function freshSdk() {
  const sdkPath = path.resolve(__dirname, "..", "index.js");
  delete require.cache[sdkPath];
  delete require.cache[path.resolve(__dirname, "..", "native.js")];
  delete require.cache[path.resolve(__dirname, "..", "errors.js")];
  return require(sdkPath);
}

test("createLimiter allows requests inside the window", async () => {
  const { createLimiter } = freshSdk();
  const limiter = createLimiter({ max: 2, window: "1s" });

  const first = await limiter.checkLimit("client-a");
  const second = await limiter.checkLimit("client-a");

  assert.equal(first.allowed, true);
  assert.equal(second.allowed, true);
});

test("createLimiter throws typed rate limit errors", async () => {
  const { createLimiter, PulsurRateLimitError } = freshSdk();
  const limiter = createLimiter({ max: 1, window: "1m" });

  await limiter.checkLimit("client-a");

  await assert.rejects(() => limiter.checkLimit("client-a"), (error) => {
    assert.ok(error instanceof PulsurRateLimitError);
    assert.equal(error.code, "PULSUR_RATE_LIMITED");
    return true;
  });
});

test("queue process worker consumes and acknowledges jobs", async () => {
  const { queue } = freshSdk();
  const emails = queue("emails");
  const processed = [];

  await emails.enqueue({ to: "user@example.com" });
  const worker = emails.process(async (payload, job) => {
    processed.push({ payload, job });
  }, { pollIntervalMs: 5 });

  await new Promise((resolve) => setTimeout(resolve, 30));
  await worker.stop();

  assert.equal(processed.length, 1);
  assert.equal(processed[0].payload.to, "user@example.com");
});

test("queue schedule keeps future jobs out of dequeue until ready", async () => {
  const { queue } = freshSdk();
  const reports = queue("reports");
  const runAt = new Date(Date.now() + 50);

  await reports.schedule({ kind: "weekly" }, runAt);

  const immediate = await reports.dequeue();
  assert.equal(immediate, null);

  await new Promise((resolve) => setTimeout(resolve, 70));
  const ready = await reports.dequeue();
  assert.equal(ready.queue, "reports");
});

test("createServer fallback listens and closes cleanly", async () => {
  const { createServer } = freshSdk();
  const server = createServer();
  const port = 38655;

  await server.listen(port);

  const response = await fetch(`http://127.0.0.1:${port}/health`);
  const body = await response.json();

  assert.equal(response.status, 200);
  assert.equal(body.ok, true);

  await server.close();
});

test("sdk uses a native rate limiter bridge when provided", async () => {
  process.env.PULSUR_RATE_LIMITER_BRIDGE = path.resolve(
    __dirname,
    "fixtures",
    "mock-rate-limiter.js",
  );
  const { createLimiter, PulsurRateLimitError } = freshSdk();
  const limiter = createLimiter({ max: 3, window: "1m" });

  const allowed = await limiter.checkLimit("allowed");
  assert.equal(allowed.allowed, true);

  await assert.rejects(() => limiter.checkLimit("blocked"), (error) => {
    assert.ok(error instanceof PulsurRateLimitError);
    return true;
  });

  delete process.env.PULSUR_RATE_LIMITER_BRIDGE;
});
