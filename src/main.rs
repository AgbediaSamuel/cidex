mod freq;
mod index;
mod mcp;
mod ngram;
mod query;
mod search;
mod store;
mod varint;
mod watcher;

use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cidex",
    about = "Indexed code search using sparse n-gram indexing"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // Search has 40+ flags by design
enum Commands {
    /// Build the search index for a directory
    Index {
        /// Path to index (defaults to current directory)
        path: Option<String>,
        /// Force full rebuild
        #[arg(long)]
        force: bool,
    },
    /// Search for a pattern
    Search {
        /// Regex pattern to search for
        pattern: String,
        /// Path to search (defaults to current directory)
        path: Option<String>,
        /// Case insensitive search
        #[arg(short)]
        i: bool,
        /// Smart case: insensitive if pattern is all lowercase
        #[arg(short = 'S', long)]
        smart_case: bool,
        /// Show only file names
        #[arg(short)]
        l: bool,
        /// Show only count of matching lines per file
        #[arg(short)]
        c: bool,
        /// Invert match: show non-matching lines
        #[arg(short)]
        v: bool,
        /// Only match whole words
        #[arg(short)]
        w: bool,
        /// Treat pattern as a literal string, not regex
        #[arg(short = 'F', long)]
        fixed_strings: bool,
        /// File type filter (e.g., py, rs, js)
        #[arg(short, long = "type")]
        t: Option<Vec<String>>,
        /// Exclude file type
        #[arg(short = 'T', long = "type-not")]
        type_not: Option<Vec<String>>,
        /// Include/exclude files by glob (prefix ! to exclude)
        #[arg(short, long)]
        glob: Option<Vec<String>>,
        /// Lines of context after match
        #[arg(short = 'A', value_name = "NUM")]
        after: Option<usize>,
        /// Lines of context before match
        #[arg(short = 'B', value_name = "NUM")]
        before: Option<usize>,
        /// Lines of context before and after match
        #[arg(short = 'C', value_name = "NUM")]
        context: Option<usize>,
        /// Limit matches per file
        #[arg(short, long)]
        max_count: Option<usize>,
        /// Max directory traversal depth
        #[arg(short = 'd', long)]
        max_depth: Option<usize>,
        /// Don't respect .gitignore
        #[arg(long)]
        no_ignore: bool,
        /// Search hidden files
        #[arg(long)]
        hidden: bool,
        /// Follow symbolic links
        #[arg(short = 'L', long)]
        follow: bool,
        /// Show files that would be searched (no searching)
        #[arg(long)]
        files: bool,
        /// Show files without matches
        #[arg(long)]
        files_without_match: bool,
        /// Print only the matched parts
        #[arg(short = 'o', long)]
        only_matching: bool,
        /// Replace matches with given text in output
        #[arg(short = 'r', long)]
        replace: Option<String>,
        /// Group matches by file with filename header
        #[arg(long)]
        heading: bool,
        /// Sort results by file path
        #[arg(long)]
        sort: bool,
        /// Print NUL byte after file paths
        #[arg(short = '0', long)]
        null: bool,
        /// Count individual matches, not matching lines
        #[arg(long)]
        count_matches: bool,
        /// Skip files larger than SIZE (e.g., 1M, 500K)
        #[arg(long)]
        max_filesize: Option<String>,
        /// Quiet: no output, exit code only
        #[arg(short = 'q', long)]
        quiet: bool,
        /// Output in JSON Lines format
        #[arg(long)]
        json: bool,
        /// Always print filenames
        #[arg(short = 'H', long)]
        with_filename: bool,
        /// Never print filenames
        #[arg(short = 'I', long)]
        no_filename: bool,
        /// Don't print line numbers
        #[arg(short = 'N', long)]
        no_line_number: bool,
        /// Show column number of first match
        #[arg(long)]
        column: bool,
        /// Vim-compatible output (file:line:column:match)
        #[arg(long)]
        vimgrep: bool,
        /// Alias for --color always --heading --line-number
        #[arg(short = 'p', long)]
        pretty: bool,
        /// Treat binary files as text
        #[arg(short = 'a', long)]
        text: bool,
        /// Reduce filtering: -u = no-ignore, -uu = +hidden, -uuu = +binary-as-text
        #[arg(short = 'u', action = clap::ArgAction::Count)]
        unrestricted: u8,
        /// Show byte offset of each matching line
        #[arg(short = 'b', long)]
        byte_offset: bool,
        /// Set context separator (default: --)
        #[arg(long, default_value = "--")]
        context_separator: String,
        /// Print both matching and non-matching lines
        #[arg(long)]
        passthru: bool,
        /// Strip leading whitespace from output
        #[arg(long)]
        trim: bool,
        /// Don't print lines longer than NUM columns (0 = no limit)
        #[arg(short = 'M', long)]
        max_columns: Option<usize>,
        /// Print stats after search
        #[arg(long)]
        stats: bool,
        /// Skip index, brute-force scan
        #[arg(long)]
        no_index: bool,
        /// Force color output
        #[arg(long)]
        color: bool,
        /// Disable color output
        #[arg(long)]
        no_color: bool,
        /// Number of threads
        #[arg(short = 'j', long)]
        threads: Option<usize>,
    },
    /// Show index status
    Status {
        /// Path (defaults to current directory)
        path: Option<String>,
    },
    /// List supported file types
    #[command(name = "type-list")]
    TypeList,
    /// Watch for file changes and auto-reindex
    Watch {
        /// Path to watch (defaults to current directory)
        path: Option<String>,
    },
    /// Start MCP server (stdio transport for AI agent integration)
    Serve,
    /// Generate shell completions
    Completions {
        /// Shell to generate for (bash, zsh, fish, powershell)
        shell: clap_complete::Shell,
    },
}

fn main() {
    let code = match run() {
        Ok(had_match) => {
            if had_match {
                0
            } else {
                1
            }
        }
        Err(e) => {
            if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                if io_err.kind() == std::io::ErrorKind::BrokenPipe {
                    0
                } else {
                    eprintln!("error: {:#}", e);
                    2
                }
            } else {
                eprintln!("error: {:#}", e);
                2
            }
        }
    };
    std::process::exit(code);
}

fn run() -> Result<bool> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Index { path, force } => {
            let root = path.unwrap_or_else(|| ".".to_string());
            let stats = index::build(Path::new(&root), force)?;
            if stats.ngram_count == 0 && stats.file_count > 0 {
                eprintln!(
                    "index up to date ({} files, {:.2}s)",
                    stats.file_count, stats.build_secs
                );
            } else {
                eprintln!(
                    "indexed {} files, {} unique n-grams in {:.2}s",
                    stats.file_count, stats.ngram_count, stats.build_secs
                );
                eprintln!(
                    "postings: {:.1} MB, lookup: {:.1} MB",
                    stats.postings_bytes as f64 / 1_048_576.0,
                    stats.lookup_bytes as f64 / 1_048_576.0,
                );
            }
            Ok(true)
        }
        Commands::Search {
            pattern,
            path,
            i: case_insensitive,
            smart_case,
            l: files_only,
            c: count,
            v: invert,
            w: word,
            fixed_strings,
            t: file_types,
            type_not,
            glob: globs,
            after,
            before,
            context,
            max_count,
            max_depth,
            only_matching,
            replace,
            heading,
            sort,
            null,
            count_matches,
            max_filesize,
            quiet,
            json,
            with_filename,
            no_filename,
            no_line_number,
            column,
            vimgrep,
            pretty,
            text,
            unrestricted,
            byte_offset,
            context_separator,
            passthru,
            trim,
            max_columns,
            no_ignore,
            hidden,
            follow,
            files,
            files_without_match,
            stats,
            no_index,
            color,
            no_color,
            threads,
        } => {
            if let Some(n) = threads {
                // Must be set before any rayon work happens
                let _ = rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build_global();
            }

            let root = path.unwrap_or_else(|| ".".to_string());
            let ctx = context.unwrap_or(0);
            let use_color = if pretty || (!no_color && color) {
                true
            } else if no_color {
                false
            } else {
                std::io::stdout().is_terminal()
            };

            let ci = if smart_case {
                !pattern.chars().any(|c| c.is_uppercase())
            } else {
                case_insensitive
            };

            // -u unrestricted levels: 1=no-ignore, 2=+hidden, 3=+binary-as-text
            let eff_no_ignore = no_ignore || unrestricted >= 1;
            let eff_hidden = hidden || unrestricted >= 2;
            let eff_text = text || unrestricted >= 3;

            let use_heading = heading || pretty;
            let show_line_numbers = !no_line_number;
            // Default is to show the filename. -H is a no-op against the default
            // (kept for ripgrep flag compatibility); -I forces it off.
            let _ = with_filename;
            let show_filename = !no_filename;

            let opts = search::SearchOpts {
                case_insensitive: ci,
                files_only,
                count,
                invert,
                word,
                fixed_strings,
                file_types: file_types.unwrap_or_default(),
                type_not: type_not.unwrap_or_default(),
                globs: globs.unwrap_or_default(),
                context_before: before.unwrap_or(ctx),
                context_after: after.unwrap_or(ctx),
                max_count,
                max_depth,
                no_ignore: eff_no_ignore,
                hidden: eff_hidden,
                follow,
                only_matching,
                replace,
                heading: use_heading,
                sort,
                null_separator: null,
                count_matches,
                max_filesize: parse_filesize(&max_filesize),
                quiet,
                json,
                show_filename,
                show_line_numbers,
                show_column: column || vimgrep,
                vimgrep,
                text: eff_text,
                byte_offset,
                context_separator,
                passthru,
                trim,
                max_columns,
                list_files: files,
                files_without_match,
                show_stats: stats,
                no_index,
                color: use_color,
            };
            search::search(Path::new(&root), &pattern, &opts)
        }
        Commands::Status { path } => {
            let root = path.unwrap_or_else(|| ".".to_string());
            match store::read_meta(Path::new(&root)) {
                Ok(meta) => {
                    let cidex_dir = index::cidex_dir(Path::new(&root));
                    let index_size: u64 = std::fs::read_dir(&cidex_dir)
                        .map(|entries| {
                            entries
                                .filter_map(|e| e.ok())
                                .filter_map(|e| e.metadata().ok())
                                .map(|m| m.len())
                                .sum()
                        })
                        .unwrap_or(0);

                    println!("version:    {}", meta.version);
                    println!("files:      {}", meta.file_count);
                    println!("tree size:  {:.1} MB", meta.tree_size as f64 / 1_048_576.0);
                    println!("index size: {:.1} MB", index_size as f64 / 1_048_576.0);
                    println!(
                        "index/src:  {:.1}%",
                        if meta.tree_size > 0 {
                            index_size as f64 / meta.tree_size as f64 * 100.0
                        } else {
                            0.0
                        }
                    );
                    println!("built:      {} (unix epoch)", meta.timestamp);
                    let age_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs().saturating_sub(meta.timestamp))
                        .unwrap_or(0);
                    println!("age:        {}s", age_secs);
                }
                Err(e) => {
                    eprintln!("no index found: {}", e);
                }
            }
            Ok(true)
        }
        Commands::Watch { path } => {
            let root = path.unwrap_or_else(|| ".".to_string());
            watcher::watch(Path::new(&root))?;
            Ok(true)
        }
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "cidex", &mut std::io::stdout());
            Ok(true)
        }
        Commands::Serve => {
            mcp::serve().ok();
            Ok(true)
        }
        Commands::TypeList => {
            let mut builder = ignore::types::TypesBuilder::new();
            builder.add_defaults();
            let types = builder.build().unwrap();
            for def in types.definitions() {
                println!("{}: {}", def.name(), def.globs().join(", "));
            }
            Ok(true)
        }
    }
}

fn parse_filesize(s: &Option<String>) -> Option<u64> {
    let s = s.as_ref()?;
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num, mult) = if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024u64)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1024 * 1024)
    } else if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1024 * 1024 * 1024)
    } else {
        (s, 1u64)
    };
    num.parse::<u64>().ok().map(|n| n * mult)
}
