#!/usr/bin/env bash
# Reproducible cidex vs ripgrep benchmarks.
#
# Usage:
#   ./benches/run.sh                 # CPython (small, fast)
#   ./benches/run.sh linux           # Linux kernel (~2 GB, slower clone)
#   ./benches/run.sh both            # both
#
# Requirements: git, hyperfine, rg (ripgrep), and a built cidex.
# We pin specific git commits so numbers are comparable across runs.

set -euo pipefail

# Pinned commits — update when re-running.
CPYTHON_COMMIT="v3.13.0"
LINUX_COMMIT="v6.11"

REPO_DIR="${REPO_DIR:-/tmp/cidex-bench}"
mkdir -p "$REPO_DIR"

CIDEX="${CIDEX:-$(pwd)/target/release/cidex}"

if [[ ! -x "$CIDEX" ]]; then
    echo "cidex binary not found at $CIDEX. Build first:"
    echo "    RUSTFLAGS=\"-C target-cpu=native\" cargo build --release"
    exit 1
fi

if ! command -v hyperfine >/dev/null 2>&1; then
    echo "hyperfine not installed. apt install hyperfine / brew install hyperfine."
    exit 1
fi

if ! command -v rg >/dev/null 2>&1; then
    echo "ripgrep (rg) not installed. apt install ripgrep / brew install ripgrep."
    exit 1
fi

clone_or_update() {
    local name="$1"
    local url="$2"
    local commit="$3"
    local dir="$REPO_DIR/$name"

    if [[ ! -d "$dir/.git" ]]; then
        echo "[$name] cloning at $commit..."
        git clone --depth 1 --branch "$commit" "$url" "$dir"
    else
        echo "[$name] already cloned"
    fi
}

bench() {
    local repo_path="$1"
    local label="$2"
    shift 2
    local query="$1"
    shift
    local rg_args=("$@")

    echo ""
    echo "--- $label ---"
    # Confirm result counts match.
    local cidex_count rg_count
    cidex_count=$("$CIDEX" search "$query" "$repo_path" "${rg_args[@]}" 2>/dev/null | wc -l)
    rg_count=$(rg "$query" "$repo_path" "${rg_args[@]}" 2>/dev/null | wc -l)
    echo "result counts: cidex=$cidex_count rg=$rg_count"
    if [[ "$cidex_count" != "$rg_count" ]]; then
        echo "  ! mismatch (will benchmark anyway)"
    fi

    hyperfine --warmup 3 --min-runs 10 \
        --command-name cidex \
        "$CIDEX search \"$query\" \"$repo_path\" ${rg_args[*]} > /dev/null 2>&1" \
        --command-name rg \
        "rg \"$query\" \"$repo_path\" ${rg_args[*]} > /dev/null 2>&1"
}

bench_repo() {
    local repo_path="$1"
    local repo_name="$2"

    echo ""
    echo "============================================================"
    echo "  Benchmarking: $repo_name ($(find "$repo_path" -type f | wc -l) files)"
    echo "============================================================"

    echo "[$repo_name] indexing..."
    "$CIDEX" index "$repo_path" --force 2>&1 | grep -v warning || true

    case "$repo_name" in
        cpython)
            bench "$repo_path" "rare: PyMem_RawCalloc" "PyMem_RawCalloc"
            bench "$repo_path" "rare: PyObject_GC_Track" "PyObject_GC_Track"
            bench "$repo_path" "medium: deprecated" "deprecated"
            bench "$repo_path" "regex: TODO.*fix" "TODO.*fix"
            bench "$repo_path" "common: return" "return"
            ;;
        linux)
            bench "$repo_path" "rare: kfree_sensitive" "kfree_sensitive"
            bench "$repo_path" "rare: ACPI_COMPANION" "ACPI_COMPANION"
            bench "$repo_path" "regex: DEFINE_MUTEX.*lock" "DEFINE_MUTEX.*lock"
            bench "$repo_path" "medium: MODULE_LICENSE" "MODULE_LICENSE"
            bench "$repo_path" "common: return" "return"
            ;;
    esac
}

run_cpython=true
run_linux=false
case "${1:-cpython}" in
    cpython) run_linux=false ;;
    linux)   run_cpython=false; run_linux=true ;;
    both)    run_linux=true ;;
    *)       echo "usage: $0 [cpython|linux|both]"; exit 1 ;;
esac

if $run_cpython; then
    clone_or_update cpython https://github.com/python/cpython "$CPYTHON_COMMIT"
    bench_repo "$REPO_DIR/cpython" cpython
fi

if $run_linux; then
    clone_or_update linux https://github.com/torvalds/linux "$LINUX_COMMIT"
    bench_repo "$REPO_DIR/linux" linux
fi

echo ""
echo "Done. Repos cached in $REPO_DIR (set REPO_DIR env var to change)."
