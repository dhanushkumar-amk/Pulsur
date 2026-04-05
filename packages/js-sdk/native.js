"use strict";

const path = require("node:path");

function tryRequire(location) {
  if (!location) {
    return null;
  }

  try {
    return require(location);
  } catch (_error) {
    return null;
  }
}

function resolveBridgePath(envKey, relativePath) {
  if (process.env[envKey]) {
    return process.env[envKey];
  }

  return path.resolve(__dirname, relativePath);
}

function loadNativeBindings() {
  return {
    httpServer: tryRequire(resolveBridgePath("PULSUR_HTTP_SERVER_BRIDGE", "../../target/debug/http_server.node")),
    rateLimiter: tryRequire(resolveBridgePath("PULSUR_RATE_LIMITER_BRIDGE", "../../target/debug/rate_limiter.node")),
    queue: tryRequire(resolveBridgePath("PULSUR_QUEUE_BRIDGE", "../../target/debug/queue.node")),
  };
}

module.exports = {
  loadNativeBindings,
};
