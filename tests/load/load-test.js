import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:3000';

export const options = {
  // Common thresholds
  thresholds: {
    http_req_failed: ['rate<0.01'], // < 1% errors
    http_req_duration: ['p(95)<500'], // 95% of requests should be < 500ms
  },
  
  // Specific Scenario Configuration
  scenarios: {
    // 1. Load Test: Ramp 0 -> 100 VUs over 60s, sustain, then ramp down.
    load: {
      executor: 'ramping-vus',
      startVUs: 0,
      stages: [
        { duration: '1m', target: 100 }, // Ramp up to 100 VUs
        { duration: '3m', target: 100 }, // Stay at 100 VUs
        { duration: '1m', target: 0 },   // Ramp down to 0 VUs
      ],
      gracefulStop: '30s',
    },
    
    // 2. Spike Test: Instantly hit 500 VUs (disabled by default, run explicitly)
    // spike: {
    //   executor: 'ramping-vus',
    //   startVUs: 0,
    //   stages: [
    //     { duration: '10s', target: 500 }, // Quick burst
    //     { duration: '1m', target: 500 },  // Sustained burst
    //     { duration: '10s', target: 0 },   // Chill out
    //   ],
    //   gracefulStop: '30s',
    // },
    
    // 3. Soak Test: 50 VUs for 1 hour (disabled by default, unless run with -s soak)
    // soak: {
    //   executor: 'constant-vus',
    //   vus: 50,
    //   duration: '1h',
    // }
  },
};

// Default (unless overridden via CLI)
if (__ENV.TEST_TYPE === 'spike') {
    options.scenarios = {
        spike: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '5s', target: 200 },
                { duration: '20s', target: 200 },
                { duration: '5s', target: 0 },
            ],
            gracefulStop: '30s',
        }
    }
} else if (__ENV.TEST_TYPE === 'soak') {
    options.scenarios = {
        soak: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '10s', target: 30 },
                { duration: '1m', target: 30 },
                { duration: '10s', target: 0 },
            ],
            gracefulStop: '30s',
        }
    }
}

export default function () {
  // Test multiple paths for balanced realism
  const responses = http.batch([
    ['GET', `${BASE_URL}/`],
    ['GET', `${BASE_URL}/api/delay`],
    ['GET', `${BASE_URL}/api/compute`],
  ]);

  responses.forEach((res) => {
    check(res, {
        'status is 200': (r) => r.status === 200,
        // When using Pulsur (Phase 51), we expect 429 intermittently if rate limited
        'not 5xx': (r) => r.status < 500,
        'has node-id': (r) => r.status !== 200 || r.headers['X-Node-Id'] !== undefined || r.headers['x-node-id'] !== undefined,
    });
  });

  sleep(1);
}
