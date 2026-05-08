# cidex

Indexed code search. The first open-source implementation of the frequency-weighted sparse n-gram indexing technique used by GitHub Code Search and Cursor.

**Status:** v0.1, early-stage.

## What it does

`cidex` builds an index of your codebase, then uses that index to skip files that can't possibly match a query before running the full regex. On large repos, it's much faster than `grep` or `ripgrep` for selective queries — function names, error messages, constants, and so on.

The algorithm picks variable-length n-grams (~3-5 bytes each) bounded by rare byte pairs. A 256x256 frequency table built from ~2 GB of source code tells the indexer which byte pairs are rare. At query time, the same table extracts covering n-grams from the search pattern, looks them up in posting lists, and intersects.

This is the same approach Cursor and GitHub use internally; every existing open-source attempt uses classical fixed-width trigrams, which produce much larger posting lists.

## Benchmarks

Linux kernel (93,785 files, 2.0 GB). All measured with `hyperfine --warmup 3`, output to `/dev/null`, release build with LTO and `target-cpu=native`. Result counts verified identical to ripgrep.

| Query | Matches | cidex vs ripgrep |
|---|---|---|
| `kfree_sensitive` | 508 | **5.5x faster** |
| `ACPI_COMPANION` | 339 | **5.1x faster** |
| `MODULE_LICENSE` | 12,613 | **2.6x faster** |
| `-w lock` | 86,615 | 1.1x faster |
| `return` (1.27M matches) | 1,268,783 | 1.1x faster |

CPython (5,498 files, 186 MB):

| Query | Matches | cidex vs ripgrep |
|---|---|---|
| `PyMem_RawCalloc` | 10 | **4.5x faster** |
| `PyObject_GC_Track` | 82 | **4.1x faster** |
| `deprecated` | 2,112 | **3.0x faster** |
| `TODO.*fix` | 4 | 1.4x faster |
| `return` (94K matches) | 94,300 | 1.3x faster |

Index builds in 8.9 seconds for the Linux kernel and takes ~30% of source size on disk.

To reproduce, see `benches/run.sh`.

## Install

```bash
# via npm (prebuilt binary, no Rust toolchain needed)
npm install -g @agbediasamuel/cidex

# or build from source
git clone https://github.com/AgbediaSamuel/cidex
cd cidex
RUSTFLAGS="-C target-cpu=native" cargo build --release
# binary at target/release/cidex
```

Supported prebuilt platforms: linux-x64, linux-arm64, darwin-arm64 (Apple Silicon), win32-x64. Intel Mac users build from source.

## Usage

```bash
# Build index (auto-runs on first search if missing)
cidex index ~/some/repo

# Search — same flags as ripgrep
cidex search "MAX_FILE_SIZE" ~/some/repo
cidex search "TODO.*fix" ~/some/repo -A 2 -B 1
cidex search "import" ~/some/repo -t py

# Watch mode — auto-reindex on file changes
cidex watch ~/some/repo

# Status of the index
cidex status ~/some/repo
```

48 ripgrep-compatible flags. Run `cidex search --help` for the full list. Highlights:
`-i` / `-S` (case), `-w` (word), `-F` (fixed strings), `-v` (invert), `-o` (only-matching),
`-r` (replace), `-l` / `-c` / `--count-matches` / `--files-without-match`,
`-t` (218 file types via `ignore` crate), `-g` (glob), `-A`/`-B`/`-C` (context),
`--json`, `--vimgrep`, `--column`, `--byte-offset`, `--passthru`, `--trim`,
`-u` (unrestricted), `--stats`, `-q` (quiet), proper exit codes.

## MCP server

cidex ships an MCP server so AI agents (Claude Code, Cursor, etc.) can call it as a tool over stdio JSON-RPC.

```bash
cidex serve
```

Exposes three tools: `cidex_search`, `cidex_index`, `cidex_status`.

Add to your Claude Code MCP config:

```json
{
  "mcpServers": {
    "cidex": {
      "command": "/path/to/cidex",
      "args": ["serve"]
    }
  }
}
```

## How it works

1. **Indexing** walks the repo respecting `.gitignore`, reads each file, computes a weight sequence using the static byte-pair frequency table, finds local maxima, and emits the n-grams between them. The (hash, file_id) pairs are sorted externally with a temp file (memory-bounded for codebases of any size), then grouped and delta-encoded as posting lists.
2. **Querying** parses the regex with `regex-syntax`, walks the HIR to extract literals, runs the same n-gram extraction on each literal, looks up the hashes in a memory-mapped lookup table via binary search, and intersects/unions the posting lists.
3. **Verification** runs the actual regex against each candidate file using the same `regex` crate as ripgrep.

For the module layout, see `src/README.md`.

## Limitations

- UTF-16 files are detected as binary and skipped (one CPython file affected in benchmarks).
- Files larger than 50 MB are excluded from the index. Files between 10-50 MB are included in the file list but searched brute-force rather than n-gram indexed.
- Short common literals (under ~12 bytes with no rare-pair structure) fall back to a full scan because they don't have enough stable interior n-grams.

## References

- Russ Cox, [Regular Expression Matching with a Trigram Index](https://swtch.com/~rsc/regexp/regexp4.html) (2012)
- GitHub Engineering, [The technology behind GitHub's new code search](https://github.blog/engineering/the-technology-behind-githubs-new-code-search/) (2023)
- Cursor, [Fast Regex Search](https://cursor.com/blog/fast-regex-search) (2025)
- Zhang et al., [An Evaluation of N-Gram Selection Strategies for Regular Expression Indexing](https://arxiv.org/html/2504.12251v2) (2025)
