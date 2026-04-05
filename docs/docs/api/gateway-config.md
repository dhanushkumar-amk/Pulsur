---
title: Gateway Configuration
description: YAML configuration reference for the current gateway crate.
---

# Gateway Configuration

The Rust gateway crate loads YAML through `GatewayConfig` and supports auth, transform, rate limit, and upstream retry settings.

Reference example:

```yaml
listen_addr: "0.0.0.0:8000"

routes:
  - method: "GET"
    path: "/api/users"
    upstream: "http://localhost:3001"

auth:
  secret: "pulsar_super_secret_HS256_key"
  bypass_paths:
    - "/api/health"

rate_limit:
  api_key_header: "x-api-key"
  limits:
    "/api": 100
    "/api/orders": 10

upstream:
  timeout_sec: 10
  max_retries: 3
  base_backoff_ms: 100

transform:
  strip_headers:
    - "x-internal-id"
  prefix_strip: "/api"
  inject_response_headers:
    Access-Control-Allow-Origin: "*"
  body_transformations:
    "legacy_username": "username"
  response_body_strip:
    - "debug_info"
```

## Top-level fields

### `listen_addr: string`

Socket address the gateway binds to.

### `routes: RouteConfig[]`

Each route declares:

- `method`
- `path`
- `upstream`

### `auth?: AuthConfig`

JWT auth settings handled by the auth plugin.

Common use:

- shared secret
- issuer or audience enforcement
- bypass paths like `/health`

### `transform?: TransformConfig`

Current transform capabilities:

- strip response headers
- strip a request path prefix
- inject response headers
- remap JSON request body fields
- remove JSON response fields

### `rate_limit?: RateLimitConfig`

Fields:

- `api_key_header`
- `limits`

Path selection uses the longest matching prefix.

### `upstream?: UpstreamConfig`

Fields used by retry logic:

- `timeout_sec`
- `max_retries`
- `base_backoff_ms`

## Operational notes

- the config file is designed for hot reload using `notify`
- runtime swapping uses `ArcSwap`
- rate limit keys resolve in this order: API key header, JWT subject, remote IP

## See also

- [`examples/gateway.yaml`](https://github.com/pulsur/pulsur/blob/main/examples/gateway.yaml)
- [Architecture Overview](../architecture/overview.md)
