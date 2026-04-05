"use strict";

const fs = require("node:fs");
const path = require("node:path");

const PLATFORM_PACKAGES = {
  "darwin-arm64": "@pulsur/http-server-darwin-arm64",
  "darwin-x64": "@pulsur/http-server-darwin-x64",
  "linux-x64": "@pulsur/http-server-linux-x64",
  "win32-x64": "@pulsur/http-server-win32-x64",
};

function readExistingManifest() {
  const manifestPath = path.join(__dirname, "..", "binary-path.json");
  if (!fs.existsSync(manifestPath)) {
    return null;
  }

  try {
    return JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  } catch (_error) {
    return null;
  }
}

function writeManifest(relativePath, source) {
  const manifestPath = path.join(__dirname, "..", "binary-path.json");
  fs.writeFileSync(
    manifestPath,
    `${JSON.stringify({
      path: relativePath,
      source,
      platform: `${process.platform}-${process.arch}`,
    }, null, 2)}\n`,
  );
}

function findLocalWorkspaceBinary() {
  const rootDir = path.resolve(__dirname, "..", "..", "..", "..");
  const platformKey = `${process.platform}-${process.arch}`;
  const platformTargets = {
    "linux-x64": "x86_64-unknown-linux-gnu",
    "darwin-x64": "x86_64-apple-darwin",
    "darwin-arm64": "aarch64-apple-darwin",
    "win32-x64": "x86_64-pc-windows-msvc",
  };
  const executableName = process.platform === "win32" ? "http_server.exe" : "http_server";
  const targetTriple = platformTargets[platformKey];

  const candidates = [];
  if (targetTriple) {
    candidates.push(path.join(rootDir, "target", targetTriple, "release", executableName));
    candidates.push(path.join(rootDir, "target", targetTriple, "debug", executableName));
  }
  candidates.push(path.join(rootDir, "target", "release", executableName));
  candidates.push(path.join(rootDir, "target", "debug", executableName));

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  return null;
}

function findPackagedBinary() {
  const executableName = process.platform === "win32" ? "pulsur-http-server.exe" : "pulsur-http-server";
  const packagedBinary = path.join(__dirname, "..", "bin", executableName);
  const manifest = readExistingManifest();
  const currentPlatform = `${process.platform}-${process.arch}`;

  if (!fs.existsSync(packagedBinary)) {
    return null;
  }

  if (manifest?.platform && manifest.platform !== currentPlatform) {
    return null;
  }

  return packagedBinary;
}

function findOptionalDependencyBinary() {
  const platformKey = `${process.platform}-${process.arch}`;
  const packageName = PLATFORM_PACKAGES[platformKey];
  if (!packageName) {
    throw new Error(`Unsupported platform for @pulsur/http-server: ${platformKey}`);
  }

  try {
    const packageJsonPath = require.resolve(`${packageName}/package.json`);
    const packageDir = path.dirname(packageJsonPath);
    const executableName = process.platform === "win32" ? "pulsur-http-server.exe" : "pulsur-http-server";
    return path.join(packageDir, "bin", executableName);
  } catch (error) {
    throw new Error(`Unable to resolve optional dependency ${packageName}: ${error.message}`);
  }
}

function main() {
  const packagedBinary = findPackagedBinary();
  if (packagedBinary) {
    writeManifest(`./bin/${path.basename(packagedBinary)}`, "local-package");
    process.stdout.write(`Using packaged binary: ${packagedBinary}\n`);
    return;
  }

  try {
    const dependencyBinary = findOptionalDependencyBinary();
    const relativePath = path.relative(path.join(__dirname, ".."), dependencyBinary).replace(/\\/g, "/");
    writeManifest(relativePath.startsWith(".") ? relativePath : `./${relativePath}`, "optional-dependency");
    process.stdout.write(`Using optional dependency binary: ${dependencyBinary}\n`);
    return;
  } catch (_error) {
    const localBinary = findLocalWorkspaceBinary();
    if (localBinary) {
      const relativePath = path.relative(path.join(__dirname, ".."), localBinary).replace(/\\/g, "/");
      writeManifest(relativePath.startsWith(".") ? relativePath : `./${relativePath}`, "workspace-build");
      process.stdout.write(`Using workspace binary: ${localBinary}\n`);
      return;
    }
  }

  process.stdout.write("Skipping binary resolution; build or install a platform package before runtime.\n");
}

main();
