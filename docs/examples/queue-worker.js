"use strict";

const assert = require("node:assert/strict");
const { queue } = require("../../packages/js-sdk");

async function run() {
  const emails = queue("docs-emails");
  const processed = [];

  await emails.enqueue({ to: "hello@example.com", subject: "Welcome" });
  const worker = emails.process(async (payload, job) => {
    processed.push({ payload, job });
  }, { pollIntervalMs: 5 });

  await new Promise((resolve) => setTimeout(resolve, 40));
  await worker.stop();

  assert.equal(processed.length, 1);
  assert.equal(processed[0].payload.to, "hello@example.com");
  console.log(`Processed ${processed.length} queue job for ${processed[0].payload.to}`);
}

if (require.main === module) {
  run().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}

module.exports = { run };
