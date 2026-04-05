/// <reference types="node" />

import { EventEmitter } from "node:events";

export type RateLimitResult = {
  allowed: boolean;
  limit: number;
  remaining: number;
  retry_after_secs: number;
  reset_after_secs: number;
};

export type QueueJob = {
  id: string;
  queue: string;
  payload_json: string;
  status: string;
  attempts: number;
  max_attempts: number;
  created_at: string;
  scheduled_at?: string | null;
};

export type QueueStats = {
  pending: number;
  processing: number;
  scheduled: number;
  completed: number;
  dead_letter: number;
};

export class PulsurError extends Error {
  code: string;
  cause?: unknown;
}

export class PulsurConnectionError extends PulsurError {}
export class PulsurTimeoutError extends PulsurError {}
export class PulsurRateLimitError extends PulsurError {
  details?: Record<string, unknown>;
}
export class PulsurNativeBindingError extends PulsurError {}

export class PulsurServer extends EventEmitter {
  port: number | null;
  listen(port: number): Promise<this>;
  close(): Promise<void>;
}

export class PulsurLimiter extends EventEmitter {
  checkLimit(key: string): Promise<RateLimitResult>;
}

export class Queue extends EventEmitter {
  constructor(name: string, options?: { url?: string; backoffBaseMs?: number; backoffMaxMs?: number });
  enqueue(payload: unknown, maxAttempts?: number): Promise<QueueJob>;
  schedule(payload: unknown, runAt: string | Date, maxAttempts?: number): Promise<QueueJob>;
  dequeue(): Promise<QueueJob | null>;
  ack(id: string): Promise<QueueJob>;
  nack(id: string, reason: string): Promise<QueueJob>;
  stats(): Promise<QueueStats>;
  process(handler: (payload: unknown, job: QueueJob) => Promise<void> | void, options?: { pollIntervalMs?: number }): Worker;
  createWorker(handler: (payload: unknown, job: QueueJob) => Promise<void> | void, options?: { pollIntervalMs?: number }): Worker;
}

export class Worker extends EventEmitter {
  stop(): Promise<void>;
}

export function createServer(): PulsurServer;
export function createLimiter(options: { max: number; window: string | number }): PulsurLimiter;
export function gateway(config: { upstream: string; plugins?: unknown[] }): { upstream: string; plugins: unknown[] };
export function queue(name: string, options?: { url?: string; backoffBaseMs?: number; backoffMaxMs?: number }): Queue;
export function rateLimit(options: { max: number; window: string | number }): PulsurLimiter;

export const Pulsur: {
  createServer: typeof createServer;
  createLimiter: typeof createLimiter;
  gateway: typeof gateway;
  queue: typeof queue;
  rateLimit: typeof rateLimit;
};
