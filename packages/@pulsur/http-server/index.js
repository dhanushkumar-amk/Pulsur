"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { spawn } = require("node:child_process");

const PLATFORM_PACKAGES = {
  "darwin-arm64": "@pulsur/http-server-darwin-arm64",
  "darwin-x64": "@pulsur/http-server-darwin-x64",
  "linux-x64": "@pulsur/http-server-linux-x64",
  "win32-x64": "@pulsur/http-server-win32-x64",
};

function readInstalledBinaryPath() {
  const manifestPath = path.join(__dirname, "binary-path.json");
  if (!fs.existsSync(manifestPath)) {
    return null;
  }

  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  return manifest?.path ? path.resolve(__dirname, manifest.path) : null;
}

function resolvePlatformBinary() {
  const platformKey = `${process.platform}-${process.arch}`;
  const packageName = PLATFORM_PACKAGES[platformKey];
  if (!packageName) {
    throw new Error(`Unsupported platform for @pulsur/http-server: ${platformKey}`);
  }

  const packageJsonPath = require.resolve(`${packageName}/package.json`);
  const packageDir = path.dirname(packageJsonPath);
  const executableName = process.platform === "win32" ? "pulsur-http-server.exe" : "pulsur-http-server";
  const binaryPath = path.join(packageDir, "bin", executableName);

  if (!fs.existsSync(binaryPath)) {
    throw new Error(`Binary package installed without executable: ${binaryPath}`);
  }

  return binaryPath;
}

function getBinaryPath() {
  const installedPath = readInstalledBinaryPath();
  if (installedPath && fs.existsSync(installedPath)) {
    return installedPath;
  }
  return resolvePlatformBinary();
}

function run(args = [], options = {}) {
  return spawn(getBinaryPath(), args, {
    stdio: "inherit",
    ...options,
  });
}

function start(args = [], options = {}) {
  return run(args, options);
}

module.exports = {
  getBinaryPath,
  run,
  start,
  get binaryPath() {
    return getBinaryPath();
  },
};
