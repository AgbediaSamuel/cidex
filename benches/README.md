# Benchmarks

`./run.sh` runs cidex against ripgrep on real codebases with `hyperfine`.

```bash
./benches/run.sh           # CPython (~5K files, fast)
./benches/run.sh linux     # Linux kernel (~93K files, slow clone)
./benches/run.sh both
```

Pinned commits in the script so numbers are comparable across runs:
- CPython at `v3.13.0`
- Linux at `v6.11`

Cached clones go to `$REPO_DIR` (defaults to `/tmp/cidex-bench`).

## What gets measured

For each query, the script:
1. Verifies cidex and ripgrep return the same result count (warns on mismatch).
2. Runs `hyperfine --warmup 3 --min-runs 10` on both, output to `/dev/null`.

The headline numbers in the README are taken from this script on a Linux x86_64 box. Your machine may vary — index size scales with corpus, query latency depends on disk speed and CPU. SIMD-aware CPUs (AVX2+) give the biggest wins because the regex crate's literal-search path uses them.

## Building cidex for fair comparison

The release profile already does LTO + single codegen unit. For best numbers also pass `-C target-cpu=native`:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

Without `target-cpu=native` you'll get more portable code that's typically 5-15% slower on rare-query benchmarks.
