# Changelog

## [0.4.0](https://github.com/dhanushkumar-amk/Pulsur/compare/v0.3.0...v0.4.0) (2026-04-07)


### Features

* initialize queue and rate-limiter crates and add CI workflow configuration ([9dd88fb](https://github.com/dhanushkumar-amk/Pulsur/commit/9dd88fb922563f53654dcbd4c136826d6caf3b83))
* scaffold http-server package and update workspace dependencies to version 0.3.0 ([b18575a](https://github.com/dhanushkumar-amk/Pulsur/commit/b18575a6ffa3df7f2d1dcd993d44b74e12f8e9c0))

## [0.3.0](https://github.com/dhanushkumar-amk/Pulsur/compare/v0.2.0...v0.3.0) (2026-04-07)


### Features

* initialize http-server crate, add npm package configuration, and implement CI workflow ([e3dd8a5](https://github.com/dhanushkumar-amk/Pulsur/commit/e3dd8a5f03aa84032791d068eafd2369e439f9be))

## [0.2.0](https://github.com/dhanushkumar-amk/Pulsur/compare/v0.1.0...v0.2.0) (2026-04-07)


### Features

* add baseline Express benchmark script and initial performance results for phases 56 and 57 ([c2f8ede](https://github.com/dhanushkumar-amk/Pulsur/commit/c2f8ede6ebb9938997173af508c65c232e7df60d))
* add pulsar-cli, implement chaos engineering test suite, and initialize load-balancer crate ([d37ba4c](https://github.com/dhanushkumar-amk/Pulsur/commit/d37ba4c586063caf468a1911b37132987689982f))
* add pulsur icon v2 asset ([0fe8e68](https://github.com/dhanushkumar-amk/Pulsur/commit/0fe8e683ba6e3d20ce16e4017cc37579305fdd27))
* implement basic HTTP server structure with router, middleware, and connection tracking ([2d73c03](https://github.com/dhanushkumar-amk/Pulsur/commit/2d73c037f9a30984d5794182e01b437258802921))
* implement CI/CD release pipeline and platform-specific binary distribution for http-server ([04c0453](https://github.com/dhanushkumar-amk/Pulsur/commit/04c04537ad64d7551cb00b34a569399f799ad30b))
* implement circuit breaker pattern with rolling window metrics and state management ([df5a931](https://github.com/dhanushkumar-amk/Pulsur/commit/df5a93174bd524fda937ff8daffd09f69b697cc1))
* implement circuit breaker pattern with rolling window metrics and state management ([345b930](https://github.com/dhanushkumar-amk/Pulsur/commit/345b9305b5ed68a96a4d85be602503e315119054))
* implement core reverse proxy engine with support for HTTP, static files, and WebSockets ([34dbfa2](https://github.com/dhanushkumar-amk/Pulsur/commit/34dbfa2794e18af2bc554f16fcd8bf244dadefde))
* implement core server components, add benchmark infrastructure, and document performance results ([5e0f3a9](https://github.com/dhanushkumar-amk/Pulsur/commit/5e0f3a9c0c5421cfc00da9e399aa472545994380))
* implement fundamental data structures and concurrency primitives in new fundamentals crate ([1dbe56f](https://github.com/dhanushkumar-amk/Pulsur/commit/1dbe56f280befd08f0a2c64ee5f794fc0374b762))
* implement gateway and http-server crates with initial routing and pipeline support ([6f98c3b](https://github.com/dhanushkumar-amk/Pulsur/commit/6f98c3bf2c63798650b548306e18e78395f599b1))
* implement gateway crate with request/response transformation and passthrough plugins ([0256a3f](https://github.com/dhanushkumar-amk/Pulsur/commit/0256a3f522da96a98a708263e591fe8e738c7d4e))
* implement gateway plugin architecture with rate limiting and request transformation modules ([7574eb9](https://github.com/dhanushkumar-amk/Pulsur/commit/7574eb92a52716517422776bc39a982676876650))
* implement gateway service and add performance benchmarking suite for http-server ([3a24cb6](https://github.com/dhanushkumar-amk/Pulsur/commit/3a24cb61efaa1cb36e5e676decef6d5bd3bbd8be))
* implement HTTP server routing engine and add futures dependency ([e8accc5](https://github.com/dhanushkumar-amk/Pulsur/commit/e8accc5ee195bc8de030e6608a7de9b918d1724e))
* implement HTTP server with TLS support, dual-port listeners, and automatic development certificate generation ([35fe82e](https://github.com/dhanushkumar-amk/Pulsur/commit/35fe82eae579f85d6791aed6d6919a49d3477bd1))
* implement HTTP server with WebSocket upgrade support and routing capabilities ([0d009bf](https://github.com/dhanushkumar-amk/Pulsur/commit/0d009bfa7a33b8915137300948c86fb9ea0a340f))
* implement http-server crate and register it in the workspace ([4ec6876](https://github.com/dhanushkumar-amk/Pulsur/commit/4ec6876836f9b86016d6074c721b451794fe39b3))
* implement http-server crate with request parsing, routing, and gzip compression support ([1ad0c95](https://github.com/dhanushkumar-amk/Pulsur/commit/1ad0c959f860e9ade1f3a47f49301402b2285b72))
* implement load balancer core logic and add integration test suite with benchmark documentation ([bc0b391](https://github.com/dhanushkumar-amk/Pulsur/commit/bc0b391a0568b72284d4e844bdab35e32248079b))
* implement minimal async HTTP/1.1 server with dual listeners, TLS support, and WebSocket upgrades ([3bc0035](https://github.com/dhanushkumar-amk/Pulsur/commit/3bc0035344abd2c22845bcb1b23972733a101c70))
* implement modular gateway architecture with hot-reloading configuration, middleware plugin support, and sliding window rate limiting. ([dbb0e1d](https://github.com/dhanushkumar-amk/Pulsur/commit/dbb0e1d09cb86efcd1cdd69847ad3fa11750b493))
* implement observability crate and scaffold dashboard project structure ([a4b664f](https://github.com/dhanushkumar-amk/Pulsur/commit/a4b664f2d42eee5ddee0fe8fe5b9eb4fe7ab1ac9))
* implement persistent job queue system with Rust backend and Node.js SDK ([0082431](https://github.com/dhanushkumar-amk/Pulsur/commit/008243100ff861af76bd247f948bc4c56714145b))
* implement Pulsar server with dual-stack support and full-stack benchmarking suite ([fc8ee00](https://github.com/dhanushkumar-amk/Pulsur/commit/fc8ee005226e9dc82207182b7224bcb56b61693c))
* implement Pulsur SDK with native Rust bindings and Node.js fallback for server, queue, and rate-limiting functionality ([52cdc8b](https://github.com/dhanushkumar-amk/Pulsur/commit/52cdc8b542ae216396ec35c86c909237bc46f684))
* implement request/response transformation and passthrough pipeline in gateway crate ([b5de74a](https://github.com/dhanushkumar-amk/Pulsur/commit/b5de74a12335bd565d0a520d567d874adafed88c))
* implement tokio-deep-dive playground with TCP servers, MPSC channels, and heartbeat demos ([190d90c](https://github.com/dhanushkumar-amk/Pulsur/commit/190d90c7f15fae7c6b0b6521f88512f91bb05b76))
* initial monorepo setup for Ferrum project ([8e06409](https://github.com/dhanushkumar-amk/Pulsur/commit/8e06409c9a1012e21df11d060c9462dfd7f6a518))
* initialize gateway crate with core dependencies and configuration ([d22f2bf](https://github.com/dhanushkumar-amk/Pulsur/commit/d22f2bf94f5710f48c8cb4d055983d472b714f13))
* initialize project structure with core engine crates, shared libraries, and gateway services ([321a2d7](https://github.com/dhanushkumar-amk/Pulsur/commit/321a2d7921ea599ca6e2220130e0f6f7296cea61))
* initialize pulsar-server crate and update workspace member paths ([dd1e663](https://github.com/dhanushkumar-amk/Pulsur/commit/dd1e663007c5996441fb6e20f5c75e6ce2512829))
* scaffold project structure with crates for load-balancer, queue, and rate-limiter, and add CI workflows. ([79f67d7](https://github.com/dhanushkumar-amk/Pulsur/commit/79f67d71909e699e03d6beb3cb158d044d65f41b))

## Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog and the file is maintained by `release-please`.
