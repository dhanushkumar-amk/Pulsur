"use strict";

const assert = require("node:assert/strict");
const { createServer } = require("../../packages/js-sdk");

async function run() {
  const server = createServer();
  const port = 38000 + Math.floor(Math.random() * 1000);

  await server.listen(port);
  console.log(`Pulsur server listening on ${server.port}`);

  const response = await fetch(`http://127.0.0.1:${port}/health`);
  const body = await response.json();

  assert.equal(response.status, 200);
  assert.equal(body.ok, true);
  console.log(`GET /health -> ${response.status} ${JSON.stringify(body)}`);

  await server.close();
}

if (require.main === module) {
  run().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}

module.exports = { run };
