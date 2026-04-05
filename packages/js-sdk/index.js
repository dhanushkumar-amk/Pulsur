"use strict";

const WebSocket = require("ws");

class Queue {
  constructor(url = "ws://127.0.0.1:6380", options = {}) {
    this.url = url;
    this.backoffBaseMs = options.backoffBaseMs ?? 100;
    this.backoffMaxMs = options.backoffMaxMs ?? 5000;
  }

  async enqueue(queue, payload, maxAttempts = 3) {
    return this.#request({
      op: "enqueue",
      queue,
      payload,
      max_attempts: maxAttempts,
    });
  }

  async schedule(queue, payload, runAt, maxAttempts = 3) {
    return this.#request({
      op: "schedule",
      queue,
      payload,
      run_at: new Date(runAt).toISOString(),
      max_attempts: maxAttempts,
    });
  }

  async cron(queue, expression, payload, maxAttempts = 3) {
    return this.#request({
      op: "cron",
      queue,
      expression,
      payload,
      max_attempts: maxAttempts,
    });
  }

  async dequeue(queue = null) {
    return this.#request({
      op: "dequeue",
      queue,
    });
  }

  async ack(id) {
    return this.#request({
      op: "ack",
      id,
    });
  }

  async nack(id, reason) {
    return this.#request({
      op: "nack",
      id,
      reason,
    });
  }

  async stats() {
    return this.#request({
      op: "stats",
    });
  }

  async subscribe(queue, onEvent) {
    const socket = await this.#connectWithRetry();
    socket.send(JSON.stringify({ op: "subscribe", queue }));

    socket.on("message", (raw) => {
      try {
        const message = JSON.parse(raw.toString());
        onEvent(message);
      } catch (_err) {
      }
    });

    return socket;
  }

  async process(queue, handler, options = {}) {
    return this.createWorker(queue, handler, options);
  }

  async createWorker(queue, handler, options = {}) {
    const concurrency = options.concurrency ?? 1;
    const workers = [];

    for (let index = 0; index < concurrency; index += 1) {
      workers.push(await this.#runWorker(queue, handler, options));
    }

    return {
      async stop() {
        await Promise.all(workers.map((worker) => worker.stop()));
      },
    };
  }

  async #runWorker(queue, handler, options) {
    let stopped = false;
    const pollIntervalMs = options.pollIntervalMs ?? 50;

    const loop = async () => {
      while (!stopped) {
        const response = await this.dequeue(queue);
        const job = response?.job ?? null;

        if (!job) {
          await delay(pollIntervalMs);
          continue;
        }

        try {
          const parsedPayload = JSON.parse(Buffer.from(job.payload).toString("utf8"));
          await handler(parsedPayload, job);
          await this.ack(job.id);
        } catch (error) {
          await this.nack(job.id, error?.message ?? "worker failure");
        }
      }
    };

    const task = loop();

    return {
      async stop() {
        stopped = true;
        await task;
      },
    };
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
            reject(new Error(response.message));
            return;
          }
          resolve(response);
        } catch (error) {
          reject(error);
        }
      });

      socket.once("error", (error) => {
        if (!settled) {
          reject(error);
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
        const maxDelay = Math.min(
          this.backoffBaseMs * (2 ** (attempt - 1)),
          this.backoffMaxMs,
        );
        const jitter = Math.floor(Math.random() * 50);
        await delay(maxDelay + jitter);
        if (attempt >= 8) {
          throw error;
        }
      }
    }
  }
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

module.exports = {
  Queue,
};
