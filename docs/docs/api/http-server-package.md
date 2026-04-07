---
title: HTTP Server npm Package
description: Packaging model for the published @pulsur/http-server package.
---

# HTTP Server npm Package

`@pulsur/http-server` is the launcher package for the packaged Rust HTTP server binary.

## Package layout

Main package:

- `packages/@pulsur/http-server`

Platform packages:

- `packages/@pulsur/http-server-linux-x64`
- `packages/@pulsur/http-server-darwin-x64`
- `packages/@pulsur/http-server-darwin-arm64`
- `packages/@pulsur/http-server-win32-x64`

## How install resolution works

1. npm installs `@pulsur/http-server`
2. the main package declares `optionalDependencies` on platform packages
3. `scripts/install.js` tries to resolve the current platform package
4. during local repo development, it can also point at a staged workspace binary
5. `index.js` returns the final executable path via `getBinaryPath()`

## API

```js
const { getBinaryPath, run, start, binaryPath } = require("@pulsur/http-server");
```

### `getBinaryPath(): string`

Returns the absolute path to the resolved executable.

### `run(args?, options?)`

Spawns the binary with `child_process.spawn`.

### `start(args?, options?)`

Alias for `run`.

### `binaryPath`

Getter that resolves the same value as `getBinaryPath()`.

## Local packaging workflow

Stage a local binary into the package:

```bash
node ./scripts/stage-npm-binary.js --component http-server --profile debug
```

Create a tarball:

```bash
npm pack ./packages/@pulsur/http-server
```

Test the tarball:

```bash
node -e "const pkg=require('@pulsur/http-server'); console.log(pkg.getBinaryPath())"
```

## When to use this package vs the JS SDK

Use `@pulsur/http-server` when you want:

- a packaged executable path
- a production-facing launcher flow
- platform-specific npm distribution

Use `@pulsur/js-sdk` when you want:

- an installable npm SDK for Node.js apps
- fallback behavior without a published binary
- access to queue and limiter helpers in one place
