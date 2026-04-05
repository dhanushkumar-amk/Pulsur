"use strict";

const { run: runGettingStarted } = require("./getting-started");
const { run: runRateLimiter } = require("./rate-limiter");
const { run: runQueueWorker } = require("./queue-worker");

async function main() {
  await runGettingStarted();
  await runRateLimiter();
  await runQueueWorker();
  console.log("All docs examples completed successfully.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
