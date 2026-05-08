// Resolves the platform-specific cidex binary path.
// The platform packages (cidex-linux-x64 etc.) are listed as
// optionalDependencies in package.json; npm only installs the one
// matching the current OS+arch.

const { existsSync } = require("fs");
const path = require("path");

const PLATFORMS = {
  "linux-x64": "cidex-linux-x64",
  "linux-arm64": "cidex-linux-arm64",
  "darwin-x64": "cidex-darwin-x64",
  "darwin-arm64": "cidex-darwin-arm64",
  "win32-x64": "cidex-win32-x64",
};

function resolveBinary() {
  const key = `${process.platform}-${process.arch}`;
  const pkg = PLATFORMS[key];
  if (!pkg) return null;

  const binaryName = process.platform === "win32" ? "cidex.exe" : "cidex";
  try {
    const pkgPath = require.resolve(`${pkg}/package.json`);
    const binPath = path.join(path.dirname(pkgPath), "bin", binaryName);
    if (existsSync(binPath)) return binPath;
  } catch (_) {
    // Platform package wasn't installed — fall through.
  }
  return null;
}

module.exports = { resolveBinary };

// When run directly via `npm install` postinstall, just verify a binary
// resolved. Don't fail the install — the wrapper script gives a useful
// error if a user actually tries to run cidex on an unsupported platform.
if (require.main === module) {
  const bin = resolveBinary();
  if (!bin) {
    console.warn(
      "cidex: no prebuilt binary for this platform. " +
        `${process.platform}-${process.arch} is not supported. ` +
        "Build from source: https://github.com/AgbediaSamuel/cidex"
    );
  }
}
