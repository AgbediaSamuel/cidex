# cidex

Indexed code search. Beats ripgrep on large codebases via frequency-weighted sparse n-gram indexing — the technique behind GitHub Code Search and Cursor's Instant Grep.

## Install

```bash
npm install -g cidex
```

This pulls down a ~5MB prebuilt binary for your platform — no Rust toolchain required.

Supported platforms: linux-x64, linux-arm64, darwin-arm64 (Apple Silicon), win32-x64.

Intel Macs aren't supported by the prebuilt binaries — build from source: https://github.com/AgbediaSamuel/cidex

## Usage

```bash
cidex index .
cidex search "MAX_FILE_SIZE"
cidex search "TODO.*fix" -A 2 -B 1
cidex watch .
```

Run `cidex --help` for the full command list, or `cidex search --help` for the 48 search flags.

## More

Full documentation: <https://github.com/AgbediaSamuel/cidex>
