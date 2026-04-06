# Pulsur 🦀

[![CI Status](https://img.shields.io/github/actions/workflow/status/pulsur/pulsur/ci.yml?branch=main)](https://github.com/pulsur/pulsur/actions)
[![NPM Version](https://img.shields.io/npm/v/@pulsur/js-sdk.svg)](https://www.npmjs.com/package/@pulsur/js-sdk)
[![License](https://img.shields.io/github/license/pulsur/pulsur.svg)](https://github.com/pulsur/pulsur/blob/main/LICENSE)
[![Discord](https://img.shields.io/discord/1234567890?label=discord)](https://discord.gg/pulsur)

**Pulsur** is a high-performance, modular infrastructure toolkit designed to bring safe, concurrent, and ultra-fast Rust power to Node.js applications. 

## 🛸 Why Pulsur?

Node.js is great for developer velocity, but sometimes you hit a wall. Pulsur provides the "heavy lifting" components built in Rust—distributed queues, rate limiters, and high-performance gateways—exposed via a simple, familiar JavaScript SDK.

> **Phase 11 Benchmark Teaser:**
> Our Rust core sustains **22,505 req/sec**—that's **7.3x faster** than the standard Node.js baseline.
> [See the full Benchmarks page](https://pulsur.dev/benchmarks)

## 🚀 Quick Example

Get a high-performance server running in seconds:

```javascript
const { HttpServer } = require('@pulsur/http-server');

const server = new HttpServer({
  port: 3000,
  workers: 4
});

server.get('/', (req, res) => {
  res.send({ hello: 'from rust-powered pulsur' });
});

server.start();
```

## 🛠️ Installation

```bash
npm install @pulsur/http-server
```

*Note: Requires Rust 1.75+ to build the native modules from source on some platforms.*

## 📁 Repository Structure

```text
pulsur/
├── crates/             # Rust Crate Workspace (The Engine)
│   ├── http-server     # High-performance server core
│   ├── gateway         # Routing and middleware
│   └── ...            # Rate limiting, Queue, etc.
├── packages/           # JavaScript Workspace (The SDK)
│   ├── js-sdk          # Primary client library
│   └── dashboard       # Next.js Management UI
└── docs/               # Documentation site (Docusaurus)
```

## 📜 Full Documentation

Visit [pulsur.dev](https://pulsur.dev) for guides, API references, and architecture details.

## 🤝 Contributing

We love contributors! Check out our [Contributing Guide](CONTRIBUTING.md) and join our community on [Discord](https://discord.gg/pulsur).

---

Distributed under the MIT License. See `LICENSE` for more information.
