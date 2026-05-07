# cidex source

Module layout. Read top to bottom — files are roughly ordered by dependency.

## `freq.rs`

Static 256x256 byte-pair weight table baked into the binary. Generated once from ~2 GB of source code (Linux kernel, CPython, Rust, VS Code, Go, Node) by `tools/generate_table.rs`. Each cell is `1 / (count + 1)` so rare byte pairs get high weights, common pairs get low weights.

Not regenerated at runtime. If you want to rebuild it, run `cargo run --bin generate-table --features devtools`.

## `varint.rs`

LEB128 encode/decode for delta-compressed file IDs in posting lists. Trivial; just `encode(u64, &mut Vec<u8>)` and `decode(&[u8], &mut usize) -> u64`.

## `ngram.rs`

The core algorithm.

- `weight_sequence(content)` — for content of length N, returns N-1 weights, one per adjacent byte pair, looked up in `freq::WEIGHT`.
- `local_maxima(weights)` — positions where the weight is a strict left max and non-strict right max. Ties break right.
- `build_all(content)` — used at index time. Returns `(hash, byte_position)` for every n-gram bounded by consecutive maxima. N-grams are variable length, averaging ~3.5 bytes.
- `build_covering(literal)` — used at query time. Returns hashes of n-grams between *strictly interior* maxima only. Edge maxima depend on context outside the literal, so we discard them. If there aren't at least two interior maxima, returns empty (caller falls back to scanning all files).
- `hash_ngram(bytes)` — multiply-shift polynomial hash. Fast, good enough for short strings, no extra crate.

## `index.rs`

Builds `.cidex/` next to the indexed root. Files written:

| File | What |
|---|---|
| `meta.bin` | 32-byte header: magic, version, file count, timestamp, tree size |
| `files.bin` | `len:u16 + path` per file. File ID is implicit (sequential). |
| `lookup.bin` | Sorted array of 24-byte records: `hash:u64 + offset:u64 + length:u32 + pad:u32`. Memory-mapped at query time, binary-searched. |
| `postings.bin` | Concatenated posting lists, each delta+varint encoded. |
| `unindexed.bin` | Sorted `u32` file IDs of files >10 MB. Always merged into candidate set so they get brute-force searched. |
| `fingerprint.bin` | FNV hash of `(path, mtime, size)` for every file. Used by incremental indexing to skip unchanged repos. |

Indexing pipeline:
1. Walk files with `ignore::WalkBuilder` (respects `.gitignore`, skips hidden).
2. Compute fingerprint. If matches stored fingerprint and not `--force`, skip rebuild.
3. Process files in chunks of 10K, parallel via rayon, extract n-grams. Files >10 MB get added to `unindexed.bin` instead. Write `(hash, file_id)` pairs sorted-per-chunk to a temp file.
4. Read pairs back, do a final sort, group by hash, delta+varint encode each posting list.
5. Write `lookup.bin` (sorted by hash), `postings.bin`, `meta.bin`, `unindexed.bin`, `fingerprint.bin`.

Memory is bounded because we never hold all postings in memory — temp file + final sort.

## `store.rs`

Index reader. Memory-maps `lookup.bin` and `postings.bin`, loads file paths and unindexed IDs into memory.

- `Index::open(root)` — opens `root/.cidex/`.
- `lookup(hash) -> Vec<u32>` — binary search in lookup.bin, decode the posting list at the referenced offset.
- `unindexed_file_ids() -> &[u32]` — file IDs that were too large to index (always brute-force searched).
- `read_meta(root)` — standalone function to read `meta.bin` for the `cidex status` command.

## `query.rs`

Regex → set of file IDs.

- `extract_literals(pattern) -> QueryPlan` — parses the regex with `regex-syntax`, walks the HIR, builds a tree of `And` / `Or` / `Literal` / `Scan` nodes. Adjacent literals in concatenations are merged. Anything we can't extract literals from (broad character classes, lookarounds, unbounded repetitions) becomes `Scan`.
- `execute(plan, index) -> Vec<u32>` — recurses on the plan. `Literal` calls `ngram::build_covering`, looks up each hash, intersects. `And` intersects child results. `Or` unions. `Scan` returns all file IDs. Always merges in `unindexed_file_ids` so large files get searched too.

## `search.rs`

End-to-end search and output formatting.

- `search(root, pattern, opts)` — top level. Builds the regex (escaping for `-F`, wrapping in `\b...\b` for `-w`), gets candidates from `query::execute` or the file walker (`--no-index` or single-file path), then in parallel reads each file, runs the regex, formats the output.
- One of three internal paths is taken per file: `search_no_context` (default, fast path with `find_iter`), `search_with_context` (when `-A`/`-B`/`-C`/`--passthru` is set), or `search_invert` (line-by-line, for `-v`). Plus `search_only_matching`, `search_json` for `-o` / `--json`.
- `format_line_full` is the shared output formatter. Handles filename, line number, column, byte offset, color highlighting, `--trim`, `--max-columns`, heading mode.
- Output streaming: each thread writes its file's results to a local `Vec<u8>` buffer, then takes a stdout mutex once to dump the whole buffer. Avoids per-line locking.
- Cross-line regex matches are filtered out to match ripgrep's line-by-line semantics (`\s` matching `\n` in `regex::bytes` would otherwise produce extra matches).

## `mcp.rs`

MCP (Model Context Protocol) server, hand-rolled JSON-RPC over stdio. About 300 lines, no `tokio` or external MCP SDK. Exposes three tools: `cidex_search`, `cidex_index`, `cidex_status`. Reads newline-delimited JSON from stdin, writes responses to stdout.

## `watcher.rs`

File watcher daemon for `cidex watch`. Uses the `notify` crate (inotify on Linux, FSEvents on macOS, ReadDirectoryChanges on Windows). Debounced 2 seconds: any file change in the watched tree marks the index dirty, and after 2 seconds of quiet it rebuilds. Skips events inside `.cidex/` itself.

## `main.rs`

Clap CLI. Subcommands: `index`, `search`, `status`, `type-list`, `watch`, `serve`, `completions`. The bulk is the `search` command's destructuring — 48 flags. Translates flags to `search::SearchOpts`. Smart-case detection (lowercase pattern → case-insensitive) and unrestricted-level mapping (`-u` → no-ignore, `-uu` → no-ignore + hidden, `-uuu` → no-ignore + hidden + binary-as-text) happen here.

Top-level `main()` handles exit codes: 0 if any match, 1 if no match, 2 on error. Broken-pipe errors are silently swallowed (so `cidex search ... | head` works).

## Dependency graph

```
freq.rs    varint.rs
   ↓          ↓
ngram.rs   index.rs
   ↓          ↓
   └─→ store.rs
          ↓
       query.rs
          ↓
       search.rs ←─── watcher.rs
          ↓               ↓
       main.rs ←──────────┘
          ↑
        mcp.rs
```

`mcp.rs` and `watcher.rs` are leaf modules called from `main.rs`. Everything else is bottom-up.
