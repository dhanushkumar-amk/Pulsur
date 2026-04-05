"use strict";

class PulsurError extends Error {
  constructor(message, code = "PULSUR_ERROR", cause) {
    super(message);
    this.name = this.constructor.name;
    this.code = code;
    this.cause = cause;
  }
}

class PulsurConnectionError extends PulsurError {
  constructor(message, cause) {
    super(message, "PULSUR_CONNECTION_ERROR", cause);
  }
}

class PulsurTimeoutError extends PulsurError {
  constructor(message, cause) {
    super(message, "PULSUR_TIMEOUT_ERROR", cause);
  }
}

class PulsurRateLimitError extends PulsurError {
  constructor(message, details = {}, cause) {
    super(message, "PULSUR_RATE_LIMITED", cause);
    this.details = details;
  }
}

class PulsurNativeBindingError extends PulsurError {
  constructor(message, cause) {
    super(message, "PULSUR_NATIVE_BINDING_ERROR", cause);
  }
}

function mapError(error, hint) {
  if (error instanceof PulsurError) {
    return error;
  }

  const message = error?.message ?? String(error);
  const normalized = `${hint ?? ""} ${message}`.toLowerCase();

  if (normalized.includes("timeout")) {
    return new PulsurTimeoutError(message, error);
  }
  if (normalized.includes("rate limit")) {
    return new PulsurRateLimitError(message, {}, error);
  }
  if (normalized.includes("native") || normalized.includes(".node")) {
    return new PulsurNativeBindingError(message, error);
  }
  if (
    normalized.includes("connect")
    || normalized.includes("socket")
    || normalized.includes("econnrefused")
  ) {
    return new PulsurConnectionError(message, error);
  }

  return new PulsurError(message, "PULSUR_ERROR", error);
}

module.exports = {
  PulsurError,
  PulsurConnectionError,
  PulsurTimeoutError,
  PulsurRateLimitError,
  PulsurNativeBindingError,
  mapError,
};
