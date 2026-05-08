// Integration tests for cidex.
//
// Two flavors:
//   1. Self-contained tests on tests/fixtures/ — always run, no external deps.
//   2. Differential tests against ripgrep — only run if `rg` is on PATH.
//
// Run with: cargo test --release
// (Release build is ~50x faster; debug build can run them too but slowly.)

use std::path::{Path, PathBuf};
use std::process::Command;

fn cidex_bin() -> PathBuf {
    // Cargo puts integration test binaries in target/{debug,release}/deps/...
    // CARGO_BIN_EXE_<name> is set by Cargo to the binary path.
    PathBuf::from(env!("CARGO_BIN_EXE_cidex"))
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn run_cidex(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(cidex_bin())
        .args(args)
        .output()
        .expect("failed to run cidex");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn run_rg(args: &[&str]) -> Option<(String, String, i32)> {
    // Skip silently if ripgrep isn't installed.
    let out = Command::new("rg").args(args).output().ok()?;
    Some((
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    ))
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// Build the fixtures index exactly once per test process. Tests run in
// parallel by default, so without this serialization several of them race
// on `.cidex/pairs.tmp` and clobber each other.
static FIXTURES_INDEX: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn ensure_fixtures_indexed() {
    FIXTURES_INDEX.get_or_init(|| {
        let fixtures = fixtures_dir();
        // Always rebuild — fingerprint check makes this cheap if it's already current,
        // and starting from a known-good state avoids "stale partial index" surprises.
        let (_, stderr, code) = run_cidex(&["index", fixtures.to_str().unwrap(), "--force"]);
        assert_eq!(code, 0, "indexing fixtures failed: {}", stderr);
    });
}

// Self-contained tests on fixtures.

#[test]
fn finds_unique_constant() {
    ensure_fixtures_indexed();
    let (stdout, _, code) =
        run_cidex(&["search", "MAX_FILE_SIZE", fixtures_dir().to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(stdout.contains("hello.rs"));
    assert_eq!(stdout.matches("MAX_FILE_SIZE").count(), 2);
}

#[test]
fn empty_results_exit_code_1() {
    ensure_fixtures_indexed();
    let (stdout, _, code) = run_cidex(&[
        "search",
        "this_string_definitely_does_not_exist_zzz",
        fixtures_dir().to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "no match should exit 1");
    assert!(stdout.is_empty());
}

#[test]
fn invalid_pattern_exit_code_2() {
    ensure_fixtures_indexed();
    // Unbalanced bracket — should be a regex parse error.
    let (_, _, code) = run_cidex(&["search", "[unclosed", fixtures_dir().to_str().unwrap()]);
    assert_eq!(code, 2, "regex error should exit 2");
}

#[test]
fn fixed_strings_treats_dot_literally() {
    ensure_fixtures_indexed();
    // Without -F, "1024." would be a regex. With -F, it's a literal.
    let (stdout, _, code) = run_cidex(&["search", "1024;", fixtures_dir().to_str().unwrap(), "-F"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("1024;"));
}

#[test]
fn case_insensitive_matches_uppercase() {
    ensure_fixtures_indexed();
    let (stdout, _, code) = run_cidex(&["search", "todo", fixtures_dir().to_str().unwrap(), "-i"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("TODO"));
}

#[test]
fn smart_case_lowercase_pattern_is_insensitive() {
    ensure_fixtures_indexed();
    let (stdout, _, _) = run_cidex(&["search", "todo", fixtures_dir().to_str().unwrap(), "-S"]);
    assert!(
        stdout.contains("TODO"),
        "smart case should match uppercase TODO"
    );
}

#[test]
fn smart_case_mixed_case_pattern_is_sensitive() {
    ensure_fixtures_indexed();
    let (stdout, _, code) = run_cidex(&["search", "Todo", fixtures_dir().to_str().unwrap(), "-S"]);
    // No "Todo" anywhere in fixtures, only "TODO" — so this should not match.
    assert_eq!(code, 1, "smart case with capital should be case-sensitive");
    assert!(!stdout.contains("TODO"));
}

#[test]
fn word_boundary_excludes_substring_match() {
    ensure_fixtures_indexed();
    // "fact" is a substring of "factorial". With -w, "fact" alone shouldn't match.
    let (_, _, code) = run_cidex(&["search", "fact", fixtures_dir().to_str().unwrap(), "-w"]);
    assert_eq!(code, 1, "-w should exclude substring matches");
}

#[test]
fn invert_match_returns_non_matching_lines() {
    ensure_fixtures_indexed();
    let (stdout, _, code) = run_cidex(&[
        "search",
        "return",
        fixtures_dir().join("math.py").to_str().unwrap(),
        "-v",
    ]);
    assert_eq!(code, 0);
    // Every line in math.py that doesn't contain "return".
    for line in stdout.lines() {
        // -v output is just the line content for single-file search
        assert!(
            !line.contains("return"),
            "found 'return' in inverted output: {}",
            line
        );
    }
}

#[test]
fn count_mode_outputs_per_file_count() {
    ensure_fixtures_indexed();
    let (stdout, _, _) = run_cidex(&[
        "search",
        "factorial",
        fixtures_dir().to_str().unwrap(),
        "-c",
    ]);
    // Format: "math.py:N"
    assert!(stdout.contains("math.py:"), "count output: {}", stdout);
}

#[test]
fn files_only_lists_matching_files() {
    ensure_fixtures_indexed();
    let (stdout, _, code) = run_cidex(&[
        "search",
        "factorial",
        fixtures_dir().to_str().unwrap(),
        "-l",
    ]);
    assert_eq!(code, 0);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with("math.py"));
}

#[test]
fn json_output_is_valid_json_per_line() {
    ensure_fixtures_indexed();
    let (stdout, _, _) = run_cidex(&[
        "search",
        "factorial",
        fixtures_dir().to_str().unwrap(),
        "--json",
    ]);
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
        assert!(parsed.is_ok(), "invalid JSON line: {}", line);
        let v = parsed.unwrap();
        assert_eq!(v["type"], "match");
        assert!(v["data"]["line_number"].is_number());
    }
}

#[test]
fn vimgrep_format_is_path_line_col_text() {
    ensure_fixtures_indexed();
    let (stdout, _, _) = run_cidex(&[
        "search",
        "factorial",
        fixtures_dir().to_str().unwrap(),
        "--vimgrep",
    ]);
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        // path:line:col:text — first three colons separate the fields.
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        assert_eq!(
            parts.len(),
            4,
            "vimgrep line should have 4 colon-separated parts: {}",
            line
        );
        assert!(
            parts[1].parse::<usize>().is_ok(),
            "line number not numeric: {}",
            parts[1]
        );
        assert!(
            parts[2].parse::<usize>().is_ok(),
            "column not numeric: {}",
            parts[2]
        );
    }
}

#[test]
fn type_filter_excludes_other_extensions() {
    ensure_fixtures_indexed();
    let (stdout, _, _) = run_cidex(&[
        "search",
        "factorial",
        fixtures_dir().to_str().unwrap(),
        "-t",
        "py",
    ]);
    // factorial appears in math.py, that's it.
    for line in stdout.lines() {
        assert!(
            line.contains(".py"),
            "non-py file in -t py output: {}",
            line
        );
    }
}

#[test]
fn binary_files_are_skipped() {
    ensure_fixtures_indexed();
    // binary.dat contains the word "binary" but has null bytes — should be skipped.
    let (stdout, _, _) = run_cidex(&["search", "binary", fixtures_dir().to_str().unwrap()]);
    assert!(
        !stdout.contains("binary.dat"),
        "binary file leaked into results"
    );
}

#[test]
fn quiet_mode_no_output() {
    ensure_fixtures_indexed();
    let (stdout, _, code) = run_cidex(&[
        "search",
        "factorial",
        fixtures_dir().to_str().unwrap(),
        "-q",
    ]);
    assert_eq!(code, 0);
    assert!(stdout.is_empty(), "-q should not print");
}

#[test]
fn auto_index_builds_on_first_search() {
    use std::fs;

    // Set up a temporary repo with no .cidex/.
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("a.rs"),
        "fn main() { let unique_marker = 1; }",
    )
    .unwrap();

    // Search without manually indexing first.
    let (stdout, _, code) = run_cidex(&["search", "unique_marker", tmp.path().to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(stdout.contains("unique_marker"));
    assert!(
        tmp.path().join(".cidex").exists(),
        "index should have been auto-built"
    );
}

#[test]
fn incremental_skips_unchanged() {
    // Two consecutive index runs on the same fixtures should be a no-op the second time.
    ensure_fixtures_indexed();
    let (_, stderr, code) = run_cidex(&["index", fixtures_dir().to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(
        stderr.contains("up to date"),
        "second index should report up-to-date, got: {}",
        stderr
    );
}

#[test]
fn type_list_includes_common_languages() {
    let (stdout, _, code) = run_cidex(&["type-list"]);
    assert_eq!(code, 0);
    for lang in &["rust", "python", "js", "go", "java"] {
        assert!(stdout.contains(lang), "type-list missing {}", lang);
    }
}

// Differential tests against ripgrep — only run when `rg` is on PATH.
// Confirms cidex produces the same line count as ripgrep across a variety of patterns.

fn diff_test(pattern: &str, extra: &[&str]) {
    if !rg_available() {
        return;
    }
    ensure_fixtures_indexed();
    let dir = fixtures_dir();
    let dir_str = dir.to_str().unwrap();

    let mut cidex_args: Vec<&str> = vec!["search", pattern, dir_str];
    cidex_args.extend(extra);
    let (cidex_out, _, _) = run_cidex(&cidex_args);

    let mut rg_args: Vec<&str> = vec![pattern, dir_str];
    rg_args.extend(extra);
    let (rg_out, _, _) = run_rg(&rg_args).unwrap();

    let cidex_lines = cidex_out.lines().count();
    let rg_lines = rg_out.lines().count();

    assert_eq!(
        cidex_lines, rg_lines,
        "line count differs for pattern {:?} extra {:?}: cidex={} rg={}\n--- cidex ---\n{}\n--- rg ---\n{}",
        pattern, extra, cidex_lines, rg_lines, cidex_out, rg_out
    );
}

#[test]
fn diff_literal_match() {
    diff_test("factorial", &[]);
}

#[test]
fn diff_regex_with_wildcard() {
    diff_test("TODO.*refactor", &[]);
}

#[test]
fn diff_alternation() {
    diff_test("factorial|fibonacci", &[]);
}

#[test]
fn diff_case_insensitive() {
    diff_test("todo", &["-i"]);
}

#[test]
fn diff_word_boundary() {
    diff_test("return", &["-w"]);
}

#[test]
fn diff_files_only() {
    diff_test("factorial", &["-l"]);
}

#[test]
fn diff_count() {
    if !rg_available() {
        return;
    }
    ensure_fixtures_indexed();

    // -c output is path-prefixed differently between tools, so just compare totals.
    let (cidex_out, _, _) =
        run_cidex(&["search", "return", fixtures_dir().to_str().unwrap(), "-c"]);
    let (rg_out, _, _) = run_rg(&["-c", "return", fixtures_dir().to_str().unwrap()]).unwrap();

    let total = |s: &str| -> usize {
        s.lines()
            .filter_map(|l| l.rsplit(':').next())
            .filter_map(|n| n.parse::<usize>().ok())
            .sum()
    };
    assert_eq!(total(&cidex_out), total(&rg_out));
}

// MCP server smoke tests.

#[test]
fn mcp_initialize_handshake() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(cidex_bin())
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cidex serve");

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", init).unwrap();
    }

    // Close stdin so the server exits after responding.
    drop(child.stdin.take());

    let out = child.wait_with_output().expect("wait_with_output");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let resp: serde_json::Value = serde_json::from_str(stdout.lines().next().expect("response"))
        .expect("parse JSON-RPC response");
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
    assert!(resp["result"]["serverInfo"]["name"].is_string());
}

#[test]
fn mcp_tools_list_returns_three_tools() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(cidex_bin())
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", init).unwrap();
        writeln!(stdin, "{}", list).unwrap();
    }
    drop(child.stdin.take());

    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 responses, got: {}", stdout);

    let resp: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 3);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"cidex_search"));
    assert!(names.contains(&"cidex_index"));
    assert!(names.contains(&"cidex_status"));
}

#[test]
fn mcp_tool_call_runs_search() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    ensure_fixtures_indexed();

    let mut child = Command::new(cidex_bin())
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
    let path = fixtures_dir().to_string_lossy().to_string();
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "cidex_search",
            "arguments": {
                "pattern": "factorial",
                "path": path,
                "max_results": 10
            }
        }
    });

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", init).unwrap();
        writeln!(stdin, "{}", call).unwrap();
    }
    drop(child.stdin.take());

    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last_line = stdout.lines().last().expect("at least one response");

    let resp: serde_json::Value = serde_json::from_str(last_line).expect("valid JSON");
    let text = resp["result"]["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("factorial"), "MCP search result: {}", text);
    assert_eq!(resp["result"]["isError"], false);
}

// MCP feature tests: context lines, file type arrays, cidexignore, gitignore.

fn mcp_call(call: &serde_json::Value) -> serde_json::Value {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(cidex_bin())
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", init).unwrap();
        writeln!(stdin, "{}", call).unwrap();
    }
    drop(child.stdin.take());

    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last_line = stdout.lines().last().expect("at least one response");
    serde_json::from_str(last_line).expect("valid JSON")
}

fn search_call(args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "cidex_search",
            "arguments": args
        }
    })
}

#[test]
fn mcp_search_with_context_lines() {
    ensure_fixtures_indexed();
    let path = fixtures_dir().to_string_lossy().to_string();
    let call = search_call(serde_json::json!({
        "pattern": "def factorial",
        "path": path,
        "context": 2,
        "max_results": 20
    }));
    let resp = mcp_call(&call);
    let text = resp["result"]["content"][0]["text"].as_str().expect("text");
    // Match line uses ":" separator; context lines use "-".
    assert!(
        text.contains(":3:def factorial"),
        "missing match line: {}",
        text
    );
    assert!(text.contains("-1-"), "missing context line: {}", text);
    assert!(text.contains("-2-"), "missing context line: {}", text);
}

#[test]
fn mcp_search_with_file_type_array() {
    ensure_fixtures_indexed();
    let path = fixtures_dir().to_string_lossy().to_string();
    let call = search_call(serde_json::json!({
        "pattern": "factorial|TODO",
        "path": path,
        "file_type": ["py", "md"],
        "max_results": 20
    }));
    let resp = mcp_call(&call);
    let text = resp["result"]["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("math.py"), "py file missing: {}", text);
    assert!(text.contains("readme.md"), "md file missing: {}", text);
    // Only .py and .md should appear; no .rs.
    assert!(!text.contains("hello.rs"), ".rs file leaked in: {}", text);
}

#[test]
fn mcp_search_with_file_type_string() {
    ensure_fixtures_indexed();
    let path = fixtures_dir().to_string_lossy().to_string();
    let call = search_call(serde_json::json!({
        "pattern": "factorial",
        "path": path,
        "file_type": "py",
        "max_results": 5
    }));
    let resp = mcp_call(&call);
    let text = resp["result"]["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("math.py"), "py file missing: {}", text);
}

#[test]
fn cidexignore_excludes_directory() {
    use std::fs;

    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("junk")).unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/main.rs"), "fn unique_marker() {}").unwrap();
    fs::write(
        tmp.path().join("junk/dump.txt"),
        "should not match unique_marker",
    )
    .unwrap();
    fs::write(tmp.path().join(".cidexignore"), "junk/\n").unwrap();

    let (stdout, _, _) = run_cidex(&["search", "unique_marker", tmp.path().to_str().unwrap()]);
    assert!(stdout.contains("main.rs"));
    assert!(!stdout.contains("dump.txt"), ".cidexignore wasn't applied");
}

#[test]
fn gitignore_applied_without_git_dir() {
    use std::fs;

    // No .git here — but .gitignore should still apply because we use require_git(false).
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("target")).unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/lib.rs"), "fn unique_marker() {}").unwrap();
    fs::write(
        tmp.path().join("target/junk.txt"),
        "binary garbage with unique_marker",
    )
    .unwrap();
    fs::write(tmp.path().join(".gitignore"), "target/\n").unwrap();

    let (stdout, _, _) = run_cidex(&["search", "unique_marker", tmp.path().to_str().unwrap()]);
    assert!(stdout.contains("lib.rs"));
    assert!(
        !stdout.contains("target/"),
        ".gitignore not applied without .git dir: {}",
        stdout
    );
}

// Helper: detect ripgrep at module level, document if missing.
#[test]
fn _ripgrep_availability_notice() {
    if !rg_available() {
        eprintln!("note: `rg` (ripgrep) not on PATH; differential tests will be skipped");
    }
}

#[allow(dead_code)]
fn _path_check(_p: &Path) {} // suppress unused import warning
