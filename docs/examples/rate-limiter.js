"use strict";

const assert = require("node:assert/strict");
const { createLimiter, PulsurRateLimitError } = require("../../packages/js-sdk");

async function run() {
  const limiter = createLimiter({ max: 2, window: "1s" });

  const first = await limiter.checkLimit("docs-client");
  const second = await limiter.checkLimit("docs-client");
  assert.equal(first.allowed, true);
  assert.equal(second.allowed, true);

  try {
    await limiter.checkLimit("docs-client");
    throw new Error("expected the third request to be rate limited");
  } catch (error) {
    assert.ok(error instanceof PulsurRateLimitError);
    assert.equal(error.code, "PULSUR_RATE_LIMITED");
    console.log(`Rate limiter rejected request with code ${error.code}`);
  }
}

if (require.main === module) {
  run().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}

module.exports = { run };
