"use strict";

const fs = require("node:fs");
const path = require("node:path");

const ROOT = path.resolve(__dirname, "..");
const PACKAGES_DIR = path.join(ROOT, "packages", "@pulsur");

const COMPONENTS = {
  "http-server": {
    packageName: "@pulsur/http-server",
    binaryBaseName: "pulsur-http-server",
    rustBinaryName: {
      win32: "pulsar-http-server.exe",
      default: "pulsar-http-server",
    },
    platforms: {
      "linux-x64": {
        packageName: "@pulsur/http-server-linux-x64",
        os: "linux",
        arch: "x64",
        target: "x86_64-unknown-linux-gnu",
      },
      "darwin-x64": {
        packageName: "@pulsur/http-server-darwin-x64",
        os: "darwin",
        arch: "x64",
        target: "x86_64-apple-darwin",
      },
      "darwin-arm64": {
        packageName: "@pulsur/http-server-darwin-arm64",
        os: "darwin",
        arch: "arm64",
        target: "aarch64-apple-darwin",
      },
      "win32-x64": {
        packageName: "@pulsur/http-server-win32-x64",
        os: "win32",
        arch: "x64",
        target: "x86_64-pc-windows-msvc",
      },
    },
  },
};

function parseArgs(argv) {
  const parsed = {};
  for (let index = 2; index < argv.length; index += 1) {
    const token = argv[index];
    if (!token.startsWith("--")) {
      continue;
    }

    const key = token.slice(2);
    const next = argv[index + 1];
    if (next && !next.startsWith("--")) {
      parsed[key] = next;
      index += 1;
      continue;
    }
    parsed[key] = true;
  }
  return parsed;
}

function ensureDir(location) {
  fs.mkdirSync(location, { recursive: true });
}

function copyBinary(source, destination) {
  if (!fs.existsSync(source)) {
    throw new Error(`Rust binary not found: ${source}`);
  }

  ensureDir(path.dirname(destination));
  fs.copyFileSync(source, destination);
}

function writeJson(location, value) {
  fs.writeFileSync(location, `${JSON.stringify(value, null, 2)}\n`);
}

function resolveLocalBinaryPath(componentConfig, profile) {
  const platformKey = `${process.platform}-${process.arch}`;
  const platformConfig = Object.values(componentConfig.platforms).find(
    (candidate) => candidate.os === process.platform && candidate.arch === process.arch,
  );

  if (!platformConfig) {
    throw new Error(`Unsupported local platform: ${platformKey}`);
  }

  const binaryName = process.platform === "win32"
    ? componentConfig.rustBinaryName.win32
    : componentConfig.rustBinaryName.default;

  const candidateProfiles = [profile];
  if (!candidateProfiles.includes("debug")) {
    candidateProfiles.push("debug");
  }

  for (const candidateProfile of candidateProfiles) {
    const targetBinary = path.join(
      ROOT,
      "target",
      platformConfig.target,
      candidateProfile,
      binaryName,
    );

    if (fs.existsSync(targetBinary)) {
      return { platformKey, platformConfig, source: targetBinary };
    }

    const fallbackBinary = path.join(ROOT, "target", candidateProfile, binaryName);
    if (fs.existsSync(fallbackBinary)) {
      return { platformKey, platformConfig, source: fallbackBinary };
    }
  }

  return {
    platformKey,
    platformConfig,
    source: path.join(ROOT, "target", profile, binaryName),
  };
}

function stageMainPackage(componentKey, profile) {
  const component = COMPONENTS[componentKey];
  if (!component) {
    throw new Error(`Unknown component: ${componentKey}`);
  }

  const { platformKey, source } = resolveLocalBinaryPath(component, profile);
  const extension = process.platform === "win32" ? ".exe" : "";
  const packageDir = path.join(PACKAGES_DIR, componentKey);
  const destination = path.join(packageDir, "bin", `${component.binaryBaseName}${extension}`);

  copyBinary(source, destination);
  writeJson(path.join(packageDir, "binary-path.json"), {
    platform: platformKey,
    path: `./bin/${path.basename(destination)}`,
    stagedBy: "scripts/stage-npm-binary.js",
  });

  return destination;
}

function stagePlatformPackage(componentKey, platformKey, profile, sourceOverride) {
  const component = COMPONENTS[componentKey];
  const platform = component?.platforms[platformKey];
  if (!component || !platform) {
    throw new Error(`Unknown component/platform combination: ${componentKey} ${platformKey}`);
  }

  const extension = platform.os === "win32" ? ".exe" : "";
  const binaryName = platform.os === "win32"
    ? component.rustBinaryName.win32
    : component.rustBinaryName.default;
  const source = sourceOverride
    ? path.resolve(ROOT, sourceOverride)
    : path.join(ROOT, "target", platform.target, profile, binaryName);
  const destination = path.join(PACKAGES_DIR, `${componentKey}-${platformKey}`, "bin", `${component.binaryBaseName}${extension}`);

  copyBinary(source, destination);
  return destination;
}

function main() {
  const args = parseArgs(process.argv);
  const componentKey = args.component;
  const profile = args.profile || "release";

  if (!componentKey) {
    throw new Error("Missing required argument: --component <name>");
  }

  if (args.platform) {
    const destination = stagePlatformPackage(componentKey, args.platform, profile, args.source);
    process.stdout.write(`${destination}\n`);
    return;
  }

  const destination = stageMainPackage(componentKey, profile);
  process.stdout.write(`${destination}\n`);
}

main();
