use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use regex::bytes::RegexBuilder;

use crate::query;
use crate::store;

#[derive(Debug, Clone)]
pub struct SearchOpts {
    pub case_insensitive: bool,
    pub files_only: bool,
    pub count: bool,
    pub invert: bool,
    pub word: bool,
    pub fixed_strings: bool,
    pub file_types: Vec<String>,
    pub type_not: Vec<String>,
    pub globs: Vec<String>,
    pub context_before: usize,
    pub context_after: usize,
    pub max_count: Option<usize>,
    pub max_depth: Option<usize>,
    pub only_matching: bool,
    pub replace: Option<String>,
    pub heading: bool,
    pub sort: bool,
    pub null_separator: bool,
    pub count_matches: bool,
    pub max_filesize: Option<u64>,
    pub quiet: bool,
    pub json: bool,
    pub show_filename: bool,
    pub show_line_numbers: bool,
    pub show_column: bool,
    pub vimgrep: bool,
    pub text: bool,
    pub byte_offset: bool,
    pub context_separator: String,
    pub passthru: bool,
    pub trim: bool,
    pub max_columns: Option<usize>,
    pub no_ignore: bool,
    pub hidden: bool,
    pub follow: bool,
    pub list_files: bool,
    pub files_without_match: bool,
    pub show_stats: bool,
    pub no_index: bool,
    #[allow(dead_code)]
    pub color: bool,
}

impl Default for SearchOpts {
    fn default() -> Self {
        SearchOpts {
            case_insensitive: false,
            files_only: false,
            count: false,
            invert: false,
            word: false,
            fixed_strings: false,
            file_types: Vec::new(),
            type_not: Vec::new(),
            globs: Vec::new(),
            context_before: 0,
            context_after: 0,
            max_count: None,
            max_depth: None,
            only_matching: false,
            replace: None,
            heading: false,
            sort: false,
            null_separator: false,
            count_matches: false,
            max_filesize: None,
            quiet: false,
            json: false,
            show_filename: true,
            show_line_numbers: true,
            show_column: false,
            vimgrep: false,
            text: false,
            byte_offset: false,
            context_separator: "--".to_string(),
            passthru: false,
            trim: false,
            max_columns: None,
            no_ignore: false,
            hidden: false,
            follow: false,
            list_files: false,
            files_without_match: false,
            show_stats: false,
            no_index: false,
            color: false,
        }
    }
}

/// Run a search and stream results to stdout. Used by the CLI.
pub fn search(root: &Path, pattern: &str, opts: &SearchOpts) -> Result<bool> {
    let mut stdout = io::stdout();
    search_to(root, pattern, opts, &mut stdout)
}

/// Run a search and stream results to any `Write`. Used by `mcp.rs` to
/// capture output into a buffer.
pub fn search_to<W: Write + Send>(
    root: &Path,
    pattern: &str,
    opts: &SearchOpts,
    out: &mut W,
) -> Result<bool> {
    let start = Instant::now();

    let effective_pattern = if opts.fixed_strings {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };
    let effective_pattern = if opts.word {
        format!(r"\b(?:{})\b", effective_pattern)
    } else {
        effective_pattern
    };

    let re = RegexBuilder::new(&effective_pattern)
        .case_insensitive(opts.case_insensitive)
        .build()
        .context("invalid regex pattern")?;

    let mut candidates = collect_candidates(root, pattern, opts)?;

    if let Some(max_size) = opts.max_filesize {
        candidates.retain(|p| {
            std::fs::metadata(p)
                .map(|m| m.len() <= max_size)
                .unwrap_or(true)
        });
    }

    if opts.sort {
        candidates.sort();
    }

    if opts.list_files {
        for path in &candidates {
            let rel = path.strip_prefix(root).unwrap_or(path);
            let _ = write!(out, "{}", rel.display());
            let _ = out.write_all(if opts.null_separator { b"\0" } else { b"\n" });
        }
        return Ok(!candidates.is_empty());
    }

    let had_match = AtomicBool::new(false);
    let out_mtx = Mutex::new(out);

    let stat_files_searched = AtomicU64::new(0);
    let stat_files_matched = AtomicU64::new(0);
    let stat_matches = AtomicU64::new(0);
    let stat_bytes = AtomicU64::new(0);

    let has_context = opts.passthru || opts.context_before > 0 || opts.context_after > 0;

    let process_file = |path: &std::path::PathBuf| -> Option<(String, Vec<u8>)> {
        let content = std::fs::read(path).ok()?;
        if !opts.text {
            let check_len = content.len().min(8192);
            if check_len > 0 && content[..check_len].contains(&0u8) {
                return None;
            }
        }

        if opts.show_stats {
            stat_files_searched.fetch_add(1, Ordering::Relaxed);
            stat_bytes.fetch_add(content.len() as u64, Ordering::Relaxed);
        }

        let rel_path = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel_path.to_string_lossy().to_string();
        let rel_bytes = rel_str.as_bytes();

        if opts.count || opts.count_matches {
            let cnt = if opts.count_matches {
                count_individual_matches(&content, &re, opts)
            } else {
                count_line_matches(&content, &re, opts)
            };
            if cnt > 0 || opts.count {
                if cnt > 0 {
                    had_match.store(true, Ordering::Relaxed);
                    if opts.show_stats {
                        stat_files_matched.fetch_add(1, Ordering::Relaxed);
                        stat_matches.fetch_add(cnt as u64, Ordering::Relaxed);
                    }
                }
                if !opts.quiet {
                    let line = format!("{}:{}\n", rel_str, cnt);
                    return Some((rel_str, line.into_bytes()));
                }
            }
            return None;
        }

        if opts.files_only || opts.files_without_match {
            let has_match = if opts.invert {
                has_non_matching_line(&content, &re)
            } else {
                re.is_match(&content)
            };

            if opts.files_without_match {
                if !has_match && !opts.quiet {
                    let sep = if opts.null_separator { "\0" } else { "\n" };
                    return Some((rel_str.clone(), format!("{}{}", rel_str, sep).into_bytes()));
                }
                return None;
            }

            if has_match {
                had_match.store(true, Ordering::Relaxed);
                if opts.show_stats {
                    stat_files_matched.fetch_add(1, Ordering::Relaxed);
                }
                if opts.quiet {
                    return None;
                }
                let sep = if opts.null_separator { "\0" } else { "\n" };
                return Some((rel_str.clone(), format!("{}{}", rel_str, sep).into_bytes()));
            }
            return None;
        }

        let mut buf = Vec::with_capacity(4096);

        let file_match_count = if opts.quiet {
            if opts.invert {
                if has_non_matching_line(&content, &re) {
                    1
                } else {
                    0
                }
            } else if re.is_match(&content) {
                1
            } else {
                0
            }
        } else if opts.json {
            search_json(&content, &re, &rel_str, opts, &mut buf)
        } else if opts.only_matching {
            search_only_matching(&content, &re, rel_bytes, opts, &mut buf)
        } else if opts.invert {
            search_invert(&content, &re, rel_bytes, opts, &mut buf)
        } else if has_context {
            search_with_context(&content, &re, rel_bytes, opts, &mut buf)
        } else {
            search_no_context(&content, &re, rel_bytes, opts, &mut buf)
        };

        if file_match_count > 0 {
            had_match.store(true, Ordering::Relaxed);
            if opts.show_stats {
                stat_files_matched.fetch_add(1, Ordering::Relaxed);
                stat_matches.fetch_add(file_match_count as u64, Ordering::Relaxed);
            }

            if opts.heading && !buf.is_empty() && !opts.json {
                let mut headed = Vec::with_capacity(rel_bytes.len() + 20 + buf.len());
                if opts.color {
                    headed.extend_from_slice(C_PATH);
                }
                headed.extend_from_slice(rel_bytes);
                if opts.color {
                    headed.extend_from_slice(C_RESET);
                }
                headed.push(b'\n');
                headed.extend_from_slice(&buf);
                headed.push(b'\n');
                return Some((rel_str, headed));
            }

            if !buf.is_empty() {
                return Some((rel_str, buf));
            }
        }

        None
    };

    if opts.sort {
        let mut results: Vec<(String, Vec<u8>)> =
            candidates.par_iter().filter_map(process_file).collect();
        results.sort_by(|a, b| a.0.cmp(&b.0));
        let mut w = out_mtx.lock().expect("output mutex poisoned");
        for (_, buf) in results {
            let _ = w.write_all(&buf);
        }
    } else {
        candidates.par_iter().for_each(|path| {
            if let Some((_, buf)) = process_file(path) {
                let mut w = out_mtx.lock().expect("output mutex poisoned");
                let _ = w.write_all(&buf);
            }
        });
    }

    if opts.show_stats {
        let elapsed = start.elapsed();
        eprintln!(
            "\n{} files searched",
            stat_files_searched.load(Ordering::Relaxed)
        );
        eprintln!(
            "{} files matched",
            stat_files_matched.load(Ordering::Relaxed)
        );
        eprintln!("{} matches", stat_matches.load(Ordering::Relaxed));
        eprintln!(
            "{:.1} MB searched in {:.3}s",
            stat_bytes.load(Ordering::Relaxed) as f64 / 1_048_576.0,
            elapsed.as_secs_f64()
        );
    }

    Ok(had_match.load(Ordering::Relaxed))
}

fn search_no_context(
    content: &[u8],
    re: &regex::bytes::Regex,
    rel_bytes: &[u8],
    opts: &SearchOpts,
    buf: &mut Vec<u8>,
) -> usize {
    let mut num_buf = itoa::Buffer::new();
    let mut prev_line_end: usize = 0;
    let mut current_line: usize = 1;
    let mut counted_up_to: usize = 0;
    let mut match_count: usize = 0;

    for m in re.find_iter(content) {
        if memchr::memchr(b'\n', &content[m.start()..m.end()]).is_some() {
            continue;
        }

        let line_start = memchr::memrchr(b'\n', &content[..m.start()])
            .map(|p| p + 1)
            .unwrap_or(0);

        // vimgrep mode emits one line per match, even on the same file line
        if !opts.vimgrep && line_start < prev_line_end && prev_line_end > 0 {
            continue;
        }

        let line_end = memchr::memchr(b'\n', &content[m.start()..])
            .map(|p| m.start() + p)
            .unwrap_or(content.len());

        for &b in &content[counted_up_to..line_start] {
            if b == b'\n' {
                current_line += 1;
            }
        }
        counted_up_to = line_start;

        let col = m.start() - line_start + 1; // 1-indexed column
        let line_bytes = &content[line_start..line_end];
        if let Some(ref repl) = opts.replace {
            let replaced = re.replace_all(line_bytes, repl.as_bytes());
            format_line(
                buf,
                rel_bytes,
                num_buf.format(current_line),
                Some(col),
                &replaced,
                line_start,
                re,
                opts,
            );
        } else {
            format_line(
                buf,
                rel_bytes,
                num_buf.format(current_line),
                Some(col),
                line_bytes,
                line_start,
                re,
                opts,
            );
        }

        prev_line_end = line_end + 1;
        match_count += 1;

        if let Some(max) = opts.max_count
            && match_count >= max
        {
            break;
        }
    }

    match_count
}

fn search_only_matching(
    content: &[u8],
    re: &regex::bytes::Regex,
    rel_bytes: &[u8],
    opts: &SearchOpts,
    buf: &mut Vec<u8>,
) -> usize {
    let mut num_buf = itoa::Buffer::new();
    let mut current_line: usize = 1;
    let mut counted_up_to: usize = 0;
    let mut match_count: usize = 0;

    for m in re.find_iter(content) {
        if memchr::memchr(b'\n', &content[m.start()..m.end()]).is_some() {
            continue;
        }

        let line_start = memchr::memrchr(b'\n', &content[..m.start()])
            .map(|p| p + 1)
            .unwrap_or(0);

        for &b in &content[counted_up_to..line_start] {
            if b == b'\n' {
                current_line += 1;
            }
        }
        counted_up_to = line_start;

        let matched = &content[m.start()..m.end()];
        if let Some(ref repl) = opts.replace {
            let replaced = re.replace(matched, repl.as_bytes());
            format_line(
                buf,
                rel_bytes,
                num_buf.format(current_line),
                None,
                &replaced,
                m.start(),
                re,
                opts,
            );
        } else {
            format_line(
                buf,
                rel_bytes,
                num_buf.format(current_line),
                None,
                matched,
                m.start(),
                re,
                opts,
            );
        }

        match_count += 1;
        if let Some(max) = opts.max_count
            && match_count >= max
        {
            break;
        }
    }

    match_count
}

fn search_invert(
    content: &[u8],
    re: &regex::bytes::Regex,
    rel_bytes: &[u8],
    opts: &SearchOpts,
    buf: &mut Vec<u8>,
) -> usize {
    let mut num_buf = itoa::Buffer::new();
    let mut line_num: usize = 1;
    let mut pos: usize = 0;
    let mut match_count: usize = 0;

    while pos < content.len() {
        let line_end = memchr::memchr(b'\n', &content[pos..])
            .map(|i| pos + i)
            .unwrap_or(content.len());

        let line = &content[pos..line_end];
        if !re.is_match(line) {
            format_line(
                buf,
                rel_bytes,
                num_buf.format(line_num),
                None,
                line,
                pos,
                re,
                opts,
            );
            match_count += 1;
            if let Some(max) = opts.max_count
                && match_count >= max
            {
                break;
            }
        }

        pos = line_end + 1;
        line_num += 1;
    }

    match_count
}

fn search_with_context(
    content: &[u8],
    re: &regex::bytes::Regex,
    rel_bytes: &[u8],
    opts: &SearchOpts,
    buf: &mut Vec<u8>,
) -> usize {
    let line_starts = build_line_starts(content);
    let total_lines = line_starts.len();

    let mut match_line_set = vec![false; total_lines];
    let mut match_count = 0;
    for m in re.find_iter(content) {
        if memchr::memchr(b'\n', &content[m.start()..m.end()]).is_some() {
            continue;
        }
        let line = offset_to_line(&line_starts, m.start());
        if !match_line_set[line] {
            match_line_set[line] = true;
            match_count += 1;
            if let Some(max) = opts.max_count
                && match_count >= max
            {
                break;
            }
        }
    }

    if match_count == 0 {
        return 0;
    }

    let ctx_before = if opts.passthru {
        usize::MAX / 2
    } else {
        opts.context_before
    };
    let ctx_after = if opts.passthru {
        usize::MAX / 2
    } else {
        opts.context_after
    };

    let mut include_set = vec![false; total_lines];
    for (line, &is_match) in match_line_set.iter().enumerate() {
        if is_match {
            let start = line.saturating_sub(ctx_before);
            let end = (line + ctx_after + 1).min(total_lines);
            include_set[start..end].fill(true);
        }
    }

    let mut num_buf = itoa::Buffer::new();
    let mut prev_included = false;
    for line in 0..total_lines {
        if !include_set[line] {
            prev_included = false;
            continue;
        }

        // Context separator between non-contiguous groups
        if !prev_included && !buf.is_empty() {
            buf.extend_from_slice(opts.context_separator.as_bytes());
            buf.push(b'\n');
        }
        prev_included = true;

        let start = line_starts[line];
        let end = line_end(content, &line_starts, line);
        let is_match = match_line_set[line];
        let sep = if is_match { b':' } else { b'-' };
        let c = opts.color;

        if opts.show_filename && !opts.heading {
            if c {
                buf.extend_from_slice(C_PATH);
            }
            buf.extend_from_slice(rel_bytes);
            if c {
                buf.extend_from_slice(C_RESET);
            }
        }
        if c {
            buf.extend_from_slice(C_SEP);
        }
        buf.push(sep);
        if c {
            buf.extend_from_slice(C_RESET);
        }
        if c {
            buf.extend_from_slice(C_NUM);
        }
        buf.extend_from_slice(num_buf.format(line + 1).as_bytes());
        if c {
            buf.extend_from_slice(C_RESET);
        }
        if c {
            buf.extend_from_slice(C_SEP);
        }
        buf.push(sep);
        if c {
            buf.extend_from_slice(C_RESET);
        }

        let line_bytes = &content[start..end];
        if is_match {
            if let Some(ref repl) = opts.replace {
                buf.extend_from_slice(&re.replace_all(line_bytes, repl.as_bytes()));
            } else if c {
                highlight_matches(buf, line_bytes, re);
            } else {
                buf.extend_from_slice(line_bytes);
            }
        } else {
            buf.extend_from_slice(line_bytes);
        }
        buf.push(b'\n');
    }

    match_count
}

fn search_json(
    content: &[u8],
    re: &regex::bytes::Regex,
    rel_path: &str,
    opts: &SearchOpts,
    buf: &mut Vec<u8>,
) -> usize {
    let mut current_line: usize = 1;
    let mut counted_up_to: usize = 0;
    let mut prev_line_end: usize = 0;
    let mut match_count: usize = 0;

    for m in re.find_iter(content) {
        if memchr::memchr(b'\n', &content[m.start()..m.end()]).is_some() {
            continue;
        }

        let line_start = memchr::memrchr(b'\n', &content[..m.start()])
            .map(|p| p + 1)
            .unwrap_or(0);

        if line_start < prev_line_end && prev_line_end > 0 {
            continue;
        }

        let line_end = memchr::memchr(b'\n', &content[m.start()..])
            .map(|p| m.start() + p)
            .unwrap_or(content.len());

        for &b in &content[counted_up_to..line_start] {
            if b == b'\n' {
                current_line += 1;
            }
        }
        counted_up_to = line_start;

        let line_text = String::from_utf8_lossy(&content[line_start..line_end]);
        let matched_text = String::from_utf8_lossy(&content[m.start()..m.end()]);

        let json_line = serde_json::json!({
            "type": "match",
            "data": {
                "path": { "text": rel_path },
                "lines": { "text": line_text },
                "line_number": current_line,
                "submatches": [{
                    "match": { "text": matched_text },
                    "start": m.start() - line_start,
                    "end": m.end() - line_start,
                }]
            }
        });
        buf.extend_from_slice(json_line.to_string().as_bytes());
        buf.push(b'\n');

        prev_line_end = line_end + 1;
        match_count += 1;

        if let Some(max) = opts.max_count
            && match_count >= max
        {
            break;
        }
    }

    match_count
}

const C_PATH: &[u8] = b"\x1b[35m";
const C_NUM: &[u8] = b"\x1b[32m";
const C_MATCH: &[u8] = b"\x1b[1;31m";
const C_RESET: &[u8] = b"\x1b[0m";
const C_SEP: &[u8] = b"\x1b[36m";

#[allow(clippy::too_many_arguments)]
fn format_line(
    buf: &mut Vec<u8>,
    rel_bytes: &[u8],
    line_num: &str,
    col: Option<usize>,
    line: &[u8],
    byte_off: usize,
    re: &regex::bytes::Regex,
    opts: &SearchOpts,
) {
    // --max-columns: skip lines that are too long
    if let Some(max) = opts.max_columns
        && max > 0
        && line.len() > max
    {
        if opts.show_filename && !opts.heading {
            buf.extend_from_slice(rel_bytes);
            buf.push(b':');
        }
        if opts.show_line_numbers {
            buf.extend_from_slice(line_num.as_bytes());
            buf.push(b':');
        }
        buf.extend_from_slice(b"[Omitted long line]\n");
        return;
    }

    let c = opts.color;
    if opts.show_filename && !opts.heading {
        if c {
            buf.extend_from_slice(C_PATH);
        }
        buf.extend_from_slice(rel_bytes);
        if c {
            buf.extend_from_slice(C_RESET);
        }
        if c {
            buf.extend_from_slice(C_SEP);
        }
        buf.push(b':');
        if c {
            buf.extend_from_slice(C_RESET);
        }
    }
    if opts.show_line_numbers {
        if c {
            buf.extend_from_slice(C_NUM);
        }
        buf.extend_from_slice(line_num.as_bytes());
        if c {
            buf.extend_from_slice(C_RESET);
        }
        if c {
            buf.extend_from_slice(C_SEP);
        }
        buf.push(b':');
        if c {
            buf.extend_from_slice(C_RESET);
        }
    }
    if let Some(col_val) = col
        && opts.show_column
    {
        if c {
            buf.extend_from_slice(C_NUM);
        }
        let mut nb = itoa::Buffer::new();
        buf.extend_from_slice(nb.format(col_val).as_bytes());
        if c {
            buf.extend_from_slice(C_RESET);
        }
        if c {
            buf.extend_from_slice(C_SEP);
        }
        buf.push(b':');
        if c {
            buf.extend_from_slice(C_RESET);
        }
    }
    if opts.byte_offset {
        if c {
            buf.extend_from_slice(C_NUM);
        }
        let mut nb = itoa::Buffer::new();
        buf.extend_from_slice(nb.format(byte_off).as_bytes());
        if c {
            buf.extend_from_slice(C_RESET);
        }
        if c {
            buf.extend_from_slice(C_SEP);
        }
        buf.push(b':');
        if c {
            buf.extend_from_slice(C_RESET);
        }
    }

    let output_line = if opts.trim {
        let trimmed = line
            .iter()
            .position(|&b| b != b' ' && b != b'\t')
            .unwrap_or(0);
        &line[trimmed..]
    } else {
        line
    };

    if c {
        highlight_matches(buf, output_line, re);
    } else {
        buf.extend_from_slice(output_line);
    }
    buf.push(b'\n');
}

fn highlight_matches(buf: &mut Vec<u8>, line: &[u8], re: &regex::bytes::Regex) {
    let mut last_end = 0;
    for m in re.find_iter(line) {
        buf.extend_from_slice(&line[last_end..m.start()]);
        buf.extend_from_slice(C_MATCH);
        buf.extend_from_slice(&line[m.start()..m.end()]);
        buf.extend_from_slice(C_RESET);
        last_end = m.end();
    }
    buf.extend_from_slice(&line[last_end..]);
}

fn count_line_matches(content: &[u8], re: &regex::bytes::Regex, opts: &SearchOpts) -> usize {
    if opts.invert {
        let mut count = 0;
        let mut pos = 0;
        while pos < content.len() {
            let line_end = memchr::memchr(b'\n', &content[pos..])
                .map(|i| pos + i)
                .unwrap_or(content.len());
            if !re.is_match(&content[pos..line_end]) {
                count += 1;
            }
            pos = line_end + 1;
            if let Some(max) = opts.max_count
                && count >= max
            {
                break;
            }
        }
        count
    } else {
        let mut count = 0;
        let mut prev_line_end = 0usize;
        for m in re.find_iter(content) {
            if memchr::memchr(b'\n', &content[m.start()..m.end()]).is_some() {
                continue;
            }
            let line_start = memchr::memrchr(b'\n', &content[..m.start()])
                .map(|p| p + 1)
                .unwrap_or(0);
            if line_start < prev_line_end && prev_line_end > 0 {
                continue;
            }
            let line_end = memchr::memchr(b'\n', &content[m.start()..])
                .map(|p| m.start() + p + 1)
                .unwrap_or(content.len());
            prev_line_end = line_end;
            count += 1;
            if let Some(max) = opts.max_count
                && count >= max
            {
                break;
            }
        }
        count
    }
}

fn count_individual_matches(content: &[u8], re: &regex::bytes::Regex, opts: &SearchOpts) -> usize {
    let mut count = 0;
    for m in re.find_iter(content) {
        if memchr::memchr(b'\n', &content[m.start()..m.end()]).is_some() {
            continue;
        }
        count += 1;
        if let Some(max) = opts.max_count
            && count >= max
        {
            break;
        }
    }
    count
}

fn has_non_matching_line(content: &[u8], re: &regex::bytes::Regex) -> bool {
    let mut pos = 0;
    while pos < content.len() {
        let line_end = memchr::memchr(b'\n', &content[pos..])
            .map(|i| pos + i)
            .unwrap_or(content.len());
        if !re.is_match(&content[pos..line_end]) {
            return true;
        }
        pos = line_end + 1;
    }
    false
}

fn collect_candidates(
    root: &Path,
    pattern: &str,
    opts: &SearchOpts,
) -> Result<Vec<std::path::PathBuf>> {
    if opts.no_index || root.is_file() {
        return collect_all_files(root, opts);
    }

    let cidex_dir = root.join(".cidex");
    if !cidex_dir.exists() {
        eprintln!("building index...");
        crate::index::build(root, false)?;
    }

    match store::Index::open(root) {
        Ok(idx) => {
            let plan = query::extract_literals(pattern)?;
            let file_ids = query::execute(&plan, &idx);
            let mut paths: Vec<_> = file_ids
                .iter()
                .map(|&id| idx.file_path(id).to_path_buf())
                .collect();

            let has_type_filter = !opts.file_types.is_empty() || !opts.type_not.is_empty();
            if has_type_filter {
                let types = build_type_matcher(opts);
                if let Some(types) = types {
                    paths.retain(|p| match types.matched(p, false) {
                        ignore::Match::None => opts.file_types.is_empty(),
                        ignore::Match::Ignore(_) => true,
                        ignore::Match::Whitelist(_) => true,
                    });
                }
            }

            if !opts.globs.is_empty() {
                paths.retain(|p| file_matches_globs(p, opts));
            }

            Ok(paths)
        }
        Err(_) => collect_all_files(root, opts),
    }
}

fn build_type_matcher(opts: &SearchOpts) -> Option<ignore::types::Types> {
    let mut builder = ignore::types::TypesBuilder::new();
    builder.add_defaults();
    for t in &opts.file_types {
        builder.select(t);
    }
    for t in &opts.type_not {
        builder.negate(t);
    }
    builder.build().ok()
}

fn file_matches_globs(path: &Path, opts: &SearchOpts) -> bool {
    for g in &opts.globs {
        if let Some(neg) = g.strip_prefix('!') {
            if glob_matches(path, neg) {
                return false;
            }
        } else if !glob_matches(path, g) {
            return false;
        }
    }
    true
}

fn glob_matches(path: &Path, pattern: &str) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if pattern.starts_with("*.") {
            return name.ends_with(&pattern[1..]);
        }
        if !pattern.contains('/') && !pattern.contains('*') {
            return name == pattern;
        }
    }
    path.to_string_lossy().contains(pattern)
}

#[inline]
fn line_end(content: &[u8], line_starts: &[usize], line: usize) -> usize {
    let start = line_starts[line];
    if line + 1 < line_starts.len() {
        let e = line_starts[line + 1];
        if e > start && content[e - 1] == b'\n' {
            if e > start + 1 && content[e - 2] == b'\r' {
                e - 2
            } else {
                e - 1
            }
        } else {
            e
        }
    } else {
        content.len()
    }
}

#[inline]
fn build_line_starts(content: &[u8]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(content.len() / 40);
    starts.push(0);
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

#[inline]
fn offset_to_line(line_starts: &[usize], byte_offset: usize) -> usize {
    match line_starts.binary_search(&byte_offset) {
        Ok(i) => i,
        Err(i) => i - 1,
    }
}

fn collect_all_files(root: &Path, opts: &SearchOpts) -> Result<Vec<std::path::PathBuf>> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(!opts.hidden)
        .git_ignore(!opts.no_ignore)
        .require_git(false)
        .add_custom_ignore_filename(".cidexignore")
        .follow_links(opts.follow)
        .filter_entry(|entry| entry.file_name() != ".cidex");

    if let Some(depth) = opts.max_depth {
        builder.max_depth(Some(depth));
    }

    if !opts.file_types.is_empty() || !opts.type_not.is_empty() {
        let mut types_builder = ignore::types::TypesBuilder::new();
        types_builder.add_defaults();
        for t in &opts.file_types {
            types_builder.select(t);
        }
        for t in &opts.type_not {
            types_builder.negate(t);
        }
        if let Ok(types) = types_builder.build() {
            builder.types(types);
        }
    }

    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if !opts.globs.is_empty() && !file_matches_globs(&path, opts) {
            continue;
        }
        files.push(path);
    }
    Ok(files)
}
