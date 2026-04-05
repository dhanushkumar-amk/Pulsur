"use strict";

const { EventEmitter } = require("node:events");
const http = require("node:http");
const createDebug = require("debug");
const WebSocket = require("ws");

const {
  PulsurConnectionError,
  PulsurError,
  PulsurRateLimitError,
  mapError,
} = require("./errors");
const { loadNativeBindings } = require("./native");

const native = loadNativeBindings();
const debug = createDebug("Pulsur:sdk");

/**
 * Create a new Pulsur HTTP server bridge.
 * Falls back to a lightweight Node server when the native addon is unavailable.
 */
function createServer() {
  const bridge = native.httpServer && typeof native.httpServer.createServer === "function"
    ? native.httpServer.createServer()
    : null;

  return new PulsurServer(bridge);
}

/**
 * Create a new Pulsur rate limiter.
 * @param {{ max: number, window: string | number }} options
 */
function createLimiter(options) {
  return new PulsurLimiter(options);
}

/**
 * Create a new Pulsur queue.
 * @param {string} name
 * @param {{ url?: string, backoffBaseMs?: number, backoffMaxMs?: number }} [options]
 */
function queue(name, options = {}) {
  return new Queue(name, options);
}

function gateway(config = {}) {
  return {
    upstream: config.upstream,
    plugins: config.plugins ?? [],
  };
}

function rateLimit(options) {
  return createLimiter(options);
}

class PulsurServer extends EventEmitter {
  constructor(nativeServer) {
    super();
    this.nativeServer = nativeServer;
    this.nodeServer = null;
    this.port = null;
    this.log = createDebug("Pulsur:http-server");
  }

  async listen(port) {
    this.log("listen(%d)", port);
    try {
      if (this.nativeServer?.listen) {
        await this.nativeServer.listen(port);
        this.port = this.nativeServer.port ?? port;
      } else {
        await new Promise((resolve, reject) => {
          this.nodeServer = http.createServer((_req, res) => {
            res.writeHead(200, { "content-type": "application/json" });
            res.end(JSON.stringify({ ok: true, message: "Pulsur fallback HTTP server" }));
          });
          this.nodeServer.once("error", reject);
          this.nodeServer.listen(port, "127.0.0.1", () => {
            this.port = port;
            resolve();
          });
        });
      }
      this.emit("connect", { port: this.port });
      return this;
    } catch (error) {
      const mapped = mapError(error, "http server listen");
      this.emit("error", mapped);
      throw mapped;
    }
  }

  async close() {
    this.log("close()");
    try {
      if (this.nativeServer?.close) {
        await this.nativeServer.close();
      } else if (this.nodeServer) {
        await new Promise((resolve, reject) => {
          this.nodeServer.close((error) => (error ? reject(error) : resolve()));
        });
        this.nodeServer = null;
      }
      this.emit("disconnect", { port: this.port });
      this.port = null;
    } catch (error) {
      const mapped = mapError(error, "http server close");
      this.emit("error", mapped);
      throw mapped;
    }
  }
}

class PulsurLimiter extends EventEmitter {
  constructor(options = {}) {
    super();
    const windowMs = parseWindow(options.window ?? "1m");
    const max = options.max ?? 100;
    this.log = createDebug("Pulsur:rate-limit");
    this.max = max;
    this.windowMs = windowMs;
    this.nativeLimiter = native.rateLimiter?.createLimiter
      ? native.rateLimiter.createLimiter(max, windowMs)
      : null;
    this.requests = new Map();
  }

  async checkLimit(key) {
    this.log("checkLimit(%s)", key);
    try {
      let status;
      if (this.nativeLimiter?.checkLimit) {
        status = await this.nativeLimiter.checkLimit(key);
      } else {
        status = fallbackCheckLimit(this.requests, key, this.max, this.windowMs);
      }

      if (!status.allowed) {
        const error = new PulsurRateLimitError("Rate limit exceeded", status);
        this.emit("error", error);
        throw error;
      }

      return status;
    } catch (error) {
      if (error instanceof PulsurRateLimitError) {
        throw error;
      }
      const mapped = mapError(error, "rate limiter check");
      this.emit("error", mapped);
      throw mapped;
    }
  }
}

class Queue extends EventEmitter {
  constructor(name, options = {}) {
    super();
    this.name = name;
    this.url = options.url ?? null;
    this.backoffBaseMs = options.backoffBaseMs ?? 100;
    this.backoffMaxMs = options.backoffMaxMs ?? 5000;
    this.log = createDebug(`Pulsur:queue:${name}`);
    this.nativeQueue = native.queue?.createQueue ? native.queue.createQueue() : null;
    this.memoryQueue = [];
    this.processing = new Map();
  }

  async enqueue(payload, maxAttempts = 3) {
    this.log("enqueue(%s)", this.name);
    try {
      const job = await this.#enqueueJob(this.name, payload, maxAttempts);
      this.emit("connect", { queue: this.name });
      return job;
    } catch (error) {
      const mapped = mapError(error, "queue enqueue");
      this.emit("error", mapped);
      throw mapped;
    }
  }

  async schedule(payload, runAt, maxAttempts = 3) {
    try {
      if (this.nativeQueue?.schedule) {
        return await this.nativeQueue.schedule(
          this.name,
          JSON.stringify(payload),
          new Date(runAt).toISOString(),
          maxAttempts,
        );
      }

      const job = makeMemoryJob(this.name, payload, maxAttempts, new Date(runAt).toISOString());
      this.memoryQueue.push(job);
      return cloneJob(job);
    } catch (error) {
      const mapped = mapError(error, "queue schedule");
      this.emit("error", mapped);
      throw mapped;
    }
  }

  async dequeue() {
    if (this.url) {
      const response = await this.#request({ op: "dequeue", queue: this.name });
      return response?.job ?? null;
    }

    if (this.nativeQueue?.dequeue) {
      return this.nativeQueue.dequeue(this.name);
    }

    this.#promoteScheduled();
    const jobIndex = this.memoryQueue.findIndex(
      (job) => job.queue === this.name && job.status === "pending",
    );
    if (jobIndex === -1) {
      return null;
    }
    const [job] = this.memoryQueue.splice(jobIndex, 1);
    job.status = "processing";
    this.processing.set(job.id, job);
    return cloneJob(job);
  }

  async ack(id) {
    if (this.url) {
      return this.#request({ op: "ack", id });
    }

    if (this.nativeQueue?.ack) {
      return this.nativeQueue.ack(id);
    }

    const job = this.processing.get(id);
    if (!job) {
      throw new PulsurError(`Job ${id} is not processing`, "PULSUR_QUEUE_STATE");
    }
    this.processing.delete(id);
    job.status = "completed";
    return cloneJob(job);
  }

  async nack(id, reason) {
    if (this.url) {
      return this.#request({ op: "nack", id, reason });
    }

    if (this.nativeQueue?.nack) {
      return this.nativeQueue.nack(id, reason);
    }

    const job = this.processing.get(id);
    if (!job) {
      throw new PulsurError(`Job ${id} is not processing`, "PULSUR_QUEUE_STATE");
    }
    this.processing.delete(id);
    job.attempts += 1;
    if (job.attempts >= job.max_attempts) {
      job.status = "dead_letter";
      return cloneJob(job);
    }
    job.status = "pending";
    this.memoryQueue.push(job);
    return cloneJob(job);
  }

  async stats() {
    if (this.url) {
      return this.#request({ op: "stats" });
    }

    if (this.nativeQueue?.stats) {
      return this.nativeQueue.stats();
    }

    this.#promoteScheduled();
    return {
      pending: this.memoryQueue.filter((job) => job.queue === this.name && job.status === "pending").length,
      processing: [...this.processing.values()].filter((job) => job.queue === this.name).length,
      scheduled: this.memoryQueue.filter((job) => job.queue === this.name && job.status === "scheduled").length,
      completed: 0,
      dead_letter: 0,
    };
  }

  process(handler, options = {}) {
    return this.createWorker(handler, options);
  }

  createWorker(handler, options = {}) {
    const worker = new Worker(this, handler, options);
    return worker;
  }

  async #enqueueJob(queueName, payload, maxAttempts) {
    if (this.url) {
      const response = await this.#request({
        op: "enqueue",
        queue: queueName,
        payload,
        max_attempts: maxAttempts,
      });
      return response.job ?? response;
    }

    if (this.nativeQueue?.enqueue) {
      return this.nativeQueue.enqueue(queueName, JSON.stringify(payload), maxAttempts);
    }

    const job = makeMemoryJob(queueName, payload, maxAttempts);
    this.memoryQueue.push(job);
    return cloneJob(job);
  }

  #promoteScheduled() {
    const now = Date.now();
    for (const job of this.memoryQueue) {
      if (job.status === "scheduled" && new Date(job.scheduled_at).getTime() <= now) {
        job.status = "pending";
      }
    }
  }

  async #request(message) {
    const socket = await this.#connectWithRetry();

    return new Promise((resolve, reject) => {
      let settled = false;

      socket.once("message", (raw) => {
        settled = true;
        socket.close();

        try {
          const response = JSON.parse(raw.toString());
          if (response.type === "error") {
            reject(new PulsurError(response.message, "PULSUR_QUEUE_REMOTE"));
            return;
          }
          resolve(response);
        } catch (error) {
          reject(error);
        }
      });

      socket.once("error", (error) => {
        if (!settled) {
          reject(new PulsurConnectionError(error.message, error));
        }
      });

      socket.send(JSON.stringify(message));
    });
  }

  async #connectWithRetry() {
    let attempt = 0;

    while (true) {
      try {
        return await connect(this.url);
      } catch (error) {
        attempt += 1;
        const maxDelay = Math.min(this.backoffBaseMs * (2 ** (attempt - 1)), this.backoffMaxMs);
        const jitter = Math.floor(Math.random() * 50);
        await delay(maxDelay + jitter);
        if (attempt >= 8) {
          throw new PulsurConnectionError(error.message, error);
        }
      }
    }
  }
}

class Worker extends EventEmitter {
  constructor(queue, handler, options = {}) {
    super();
    this.queue = queue;
    this.handler = handler;
    this.pollIntervalMs = options.pollIntervalMs ?? 25;
    this.stopped = false;
    this.log = createDebug(`Pulsur:worker:${queue.name}`);
    this.task = this.#loop();
  }

  async stop() {
    this.stopped = true;
    await this.task;
    this.emit("disconnect", { queue: this.queue.name });
  }

  async #loop() {
    this.emit("connect", { queue: this.queue.name });
    while (!this.stopped) {
      const job = await this.queue.dequeue();
      if (!job) {
        await delay(this.pollIntervalMs);
        continue;
      }

      try {
        const payload = typeof job.payload_json === "string"
          ? JSON.parse(job.payload_json)
          : job.payload_json;
        await this.handler(payload, job);
        await this.queue.ack(job.id);
      } catch (error) {
        const mapped = mapError(error, "worker processing");
        this.emit("error", mapped);
        await this.queue.nack(job.id, mapped.message);
      }
    }
  }
}

function fallbackCheckLimit(store, key, max, windowMs) {
  const now = Date.now();
  const entries = (store.get(key) ?? []).filter((timestamp) => now - timestamp < windowMs);
  const allowed = entries.length < max;

  if (allowed) {
    entries.push(now);
  }

  store.set(key, entries);
  const oldest = entries[0] ?? now;
  const retryAfterMs = allowed ? 0 : Math.max(windowMs - (now - oldest), 0);

  return {
    allowed,
    limit: max,
    remaining: Math.max(max - entries.length, 0),
    retry_after_secs: Math.ceil(retryAfterMs / 1000),
    reset_after_secs: Math.ceil(retryAfterMs / 1000),
  };
}

function parseWindow(window) {
  if (typeof window === "number") {
    return window;
  }
  if (typeof window !== "string") {
    return 60_000;
  }

  const trimmed = window.trim().toLowerCase();
  if (trimmed.endsWith("ms")) {
    return Number.parseInt(trimmed, 10);
  }
  if (trimmed.endsWith("s")) {
    return Number.parseInt(trimmed, 10) * 1000;
  }
  if (trimmed.endsWith("m")) {
    return Number.parseInt(trimmed, 10) * 60_000;
  }
  return Number.parseInt(trimmed, 10);
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.once("open", () => resolve(socket));
    socket.once("error", reject);
  });
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function makeMemoryJob(queueName, payload, maxAttempts, scheduledAt = null) {
  return {
    id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
    queue: queueName,
    payload_json: JSON.stringify(payload),
    status: scheduledAt ? "scheduled" : "pending",
    attempts: 0,
    max_attempts: maxAttempts,
    created_at: new Date().toISOString(),
    scheduled_at: scheduledAt,
  };
}

function cloneJob(job) {
  return JSON.parse(JSON.stringify(job));
}

module.exports = {
  Pulsur: {
    createServer,
    createLimiter,
    gateway,
    queue,
    rateLimit,
  },
  createServer,
  createLimiter,
  gateway,
  queue,
  rateLimit,
  Queue,
  Worker,
  PulsurServer,
  PulsurLimiter,
  ...require("./errors"),
};
