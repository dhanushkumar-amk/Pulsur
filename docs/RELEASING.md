# Releasing

This repo currently exposes two npm deliverables that other users can install:

- `@pulsur/js-sdk`
- `@pulsur/http-server` plus its platform packages:
  `@pulsur/http-server-linux-x64`,
  `@pulsur/http-server-darwin-x64`,
  `@pulsur/http-server-darwin-arm64`,
  `@pulsur/http-server-win32-x64`

## Before the first publish

1. Create an npm account at <https://www.npmjs.com/>.
2. Run `npm login`.
3. Verify the account with `npm whoami`.
4. If the `@pulsur` scope belongs to an org, make sure your npm user has publish rights for that scope.
5. Store an automation token in GitHub as `NPM_TOKEN` if you want CI publishing later.

## Release checklist

1. Make sure all package versions match.
2. Run `cargo test --workspace`.
3. Run `npm test --workspaces --if-present`.
4. Build the release Rust binary for each platform package you plan to publish.
5. Stage the platform package binary. For Windows x64, run:
   `node ./scripts/stage-npm-binary.js --component http-server --platform win32-x64 --profile release`
6. Stage the local binary for the launcher package:
   `node ./scripts/stage-npm-binary.js --component http-server --profile release`
7. Sanity-check the tarballs:
   `npm pack ./packages/js-sdk`
   `npm pack ./packages/@pulsur/http-server`
8. In a clean test folder, install the tarballs and verify:
   `node -e "const sdk=require('@pulsur/js-sdk'); console.log(typeof sdk.createServer)"`
   `node -e "const pkg=require('@pulsur/http-server'); console.log(pkg.getBinaryPath())"`

## Manual publish order

Publish in this exact order so installs work for users immediately:

1. `npm publish ./packages/js-sdk --access public`
2. `npm publish ./packages/@pulsur/http-server-linux-x64 --access public`
3. `npm publish ./packages/@pulsur/http-server-darwin-x64 --access public`
4. `npm publish ./packages/@pulsur/http-server-darwin-arm64 --access public`
5. `npm publish ./packages/@pulsur/http-server-win32-x64 --access public`
6. `npm publish ./packages/@pulsur/http-server --access public`

You can also use the repo scripts:

- `npm run release:publish-js-sdk`
- `npm run release:publish-http-server`
- `npm run release:publish-packages`

## How users install the packages

### JS SDK

```bash
npm install @pulsur/js-sdk
```

```js
const { createServer, createLimiter, queue } = require("@pulsur/js-sdk");
```

### HTTP server binary package

```bash
npm install @pulsur/http-server
```

npm will pull the matching optional platform package automatically.

```js
const { getBinaryPath, run } = require("@pulsur/http-server");

console.log(getBinaryPath());
run(["--help"]);
```

## Versioning rules

- `feat:` creates a minor release.
- `fix:` creates a patch release.
- `BREAKING CHANGE:` creates a major release.
- All Rust crates stay aligned through `version.workspace` in the root `Cargo.toml`.
