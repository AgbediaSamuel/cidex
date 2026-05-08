use std::io::{self, BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};

use crate::search::SearchOpts;

pub fn serve() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();

        // Notifications (no id) — just acknowledge silently
        if id.is_none() {
            continue;
        }

        let response = match method {
            "initialize" => handle_initialize(&id),
            "tools/list" => handle_tools_list(&id),
            "tools/call" => handle_tools_call(&msg, &id),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {}", method) }
            }),
        };

        let resp_str = serde_json::to_string(&response).unwrap();
        writeln!(out, "{}", resp_str)?;
        out.flush()?;
    }

    Ok(())
}

fn handle_initialize(id: &Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": "2025-06-18",
            "capabilities": {
                "tools": { "listChanged": false }
            },
            "serverInfo": {
                "name": "cidex-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

fn handle_tools_list(id: &Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                {
                    "name": "cidex_search",
                    "description": "Search for a regex pattern in an indexed codebase. Uses frequency-weighted sparse n-gram indexing for sub-millisecond results on large codebases. Much faster than grep/ripgrep for selective queries.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "pattern": {
                                "type": "string",
                                "description": "Regex pattern to search for"
                            },
                            "path": {
                                "type": "string",
                                "description": "Root directory to search (must be indexed with cidex index)"
                            },
                            "case_insensitive": {
                                "type": "boolean",
                                "description": "Case insensitive search",
                                "default": false
                            },
                            "fixed_strings": {
                                "type": "boolean",
                                "description": "Treat pattern as literal, not regex",
                                "default": false
                            },
                            "word": {
                                "type": "boolean",
                                "description": "Match whole words only",
                                "default": false
                            },
                            "file_type": {
                                "description": "Filter by file type. Either a string (e.g. \"py\") or an array (e.g. [\"py\",\"toml\"]). Run `cidex type-list` for the full list.",
                                "oneOf": [
                                    { "type": "string" },
                                    { "type": "array", "items": { "type": "string" } }
                                ]
                            },
                            "before": {
                                "type": "integer",
                                "description": "Lines of context before each match",
                                "default": 0
                            },
                            "after": {
                                "type": "integer",
                                "description": "Lines of context after each match",
                                "default": 0
                            },
                            "context": {
                                "type": "integer",
                                "description": "Lines of context before AND after each match (overrides before/after if set)",
                                "default": 0
                            },
                            "max_results": {
                                "type": "integer",
                                "description": "Maximum number of matching lines to return total",
                                "default": 100
                            },
                            "files_only": {
                                "type": "boolean",
                                "description": "Return only file paths, not matching lines",
                                "default": false
                            }
                        },
                        "required": ["pattern"]
                    }
                },
                {
                    "name": "cidex_index",
                    "description": "Build a search index for a directory. Must be run before cidex_search.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Directory to index"
                            },
                            "force": {
                                "type": "boolean",
                                "description": "Force full rebuild",
                                "default": false
                            }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "cidex_status",
                    "description": "Show index status: file count, size, freshness.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Directory to check"
                            }
                        },
                        "required": ["path"]
                    }
                }
            ]
        }
    })
}

fn handle_tools_call(msg: &Value, id: &Option<Value>) -> Value {
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    match tool_name {
        "cidex_search" => call_search(&args, id),
        "cidex_index" => call_index(&args, id),
        "cidex_status" => call_status(&args, id),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32602, "message": format!("Unknown tool: {}", tool_name) }
        }),
    }
}

fn call_search(args: &Value, id: &Option<Value>) -> Value {
    let pattern = args.get("pattern").and_then(|p| p.as_str()).unwrap_or("");
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;
    let file_types = parse_file_types(args.get("file_type"));
    let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let before = args.get("before").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let after = args.get("after").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    let opts = SearchOpts {
        case_insensitive: args
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        fixed_strings: args
            .get("fixed_strings")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        word: args.get("word").and_then(|v| v.as_bool()).unwrap_or(false),
        files_only: args
            .get("files_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        file_types,
        context_before: if context > 0 { context } else { before },
        context_after: if context > 0 { context } else { after },
        max_count: Some(max_results),
        ..SearchOpts::default()
    };

    let mut output = Vec::new();
    match crate::search::search_to(Path::new(path), pattern, &opts, &mut output) {
        Ok(_) => {
            // SearchOpts::max_count is per-file (ripgrep convention).
            // For MCP we want a total cap — truncate after N lines.
            let truncated = truncate_to_lines(&output, max_results);
            let text = String::from_utf8_lossy(truncated).to_string();
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }
            })
        }
        Err(e) => tool_error(id, &format!("Search failed: {}", e)),
    }
}

fn truncate_to_lines(buf: &[u8], max_lines: usize) -> &[u8] {
    let mut lines = 0;
    for (i, &b) in buf.iter().enumerate() {
        if b == b'\n' {
            lines += 1;
            if lines >= max_lines {
                return &buf[..=i];
            }
        }
    }
    buf
}

fn call_index(args: &Value, id: &Option<Value>) -> Value {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    match crate::index::build(Path::new(path), force) {
        Ok(stats) => {
            let text = format!(
                "Indexed {} files, {} unique n-grams in {:.2}s\npostings: {:.1} MB, lookup: {:.1} MB",
                stats.file_count,
                stats.ngram_count,
                stats.build_secs,
                stats.postings_bytes as f64 / 1_048_576.0,
                stats.lookup_bytes as f64 / 1_048_576.0,
            );
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }
            })
        }
        Err(e) => tool_error(id, &format!("Index failed: {}", e)),
    }
}

fn call_status(args: &Value, id: &Option<Value>) -> Value {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");

    match crate::store::read_meta(Path::new(path)) {
        Ok(meta) => {
            let cidex_dir = crate::index::cidex_dir(Path::new(path));
            let index_size: u64 = std::fs::read_dir(&cidex_dir)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter_map(|e| e.metadata().ok())
                        .map(|m| m.len())
                        .sum()
                })
                .unwrap_or(0);

            let text = format!(
                "files: {}\ntree size: {:.1} MB\nindex size: {:.1} MB\nindex/src: {:.1}%",
                meta.file_count,
                meta.tree_size as f64 / 1_048_576.0,
                index_size as f64 / 1_048_576.0,
                if meta.tree_size > 0 {
                    index_size as f64 / meta.tree_size as f64 * 100.0
                } else {
                    0.0
                }
            );
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }
            })
        }
        Err(e) => tool_error(id, &format!("No index found: {}", e)),
    }
}

/// Parse `file_type` arg as either a string or an array of strings.
fn parse_file_types(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn tool_error(id: &Option<Value>, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "isError": true
        }
    })
}
