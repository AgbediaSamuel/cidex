#!/usr/bin/env node
// Resolve the platform-specific cidex binary and exec it with our argv.

const { spawnSync } = require("child_process");
const { resolveBinary } = require("../install.js");

const binary = resolveBinary();
if (!binary) {
  console.error(
    "cidex: no prebuilt binary available for this platform.\n" +
      "Supported: linux-x64, linux-arm64, darwin-arm64, win32-x64.\n" +
      "Build from source: https://github.com/AgbediaSamuel/cidex"
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), {
  stdio: "inherit",
});

if (result.error) {
  console.error("cidex: failed to spawn binary:", result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 0);
