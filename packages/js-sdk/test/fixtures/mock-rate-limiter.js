"use strict";

class MockLimiter {
  constructor(max, windowMs) {
    this.max = max;
    this.windowMs = windowMs;
    this.calls = 0;
  }

  async checkLimit(key) {
    this.calls += 1;
    return {
      allowed: key !== "blocked",
      limit: this.max,
      remaining: key === "blocked" ? 0 : this.max - this.calls,
      retry_after_secs: key === "blocked" ? 1 : 0,
      reset_after_secs: key === "blocked" ? 1 : 0,
    };
  }
}

module.exports = {
  createLimiter(max, windowMs) {
    return new MockLimiter(max, windowMs);
  },
};
