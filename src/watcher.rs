use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};

use crate::index;

pub fn watch(root: &Path) -> Result<()> {
    let root = root.canonicalize().context("failed to resolve path")?;
    let cidex_dir = index::cidex_dir(&root);

    if !cidex_dir.exists() {
        eprintln!("building initial index...");
        let stats = index::build(&root, false)?;
        eprintln!(
            "indexed {} files, {} n-grams in {:.2}s",
            stats.file_count, stats.ngram_count, stats.build_secs
        );
    }

    eprintln!("watching {} for changes (Ctrl+C to stop)", root.display());

    let dirty = Arc::new(AtomicBool::new(false));
    let dirty_clone = dirty.clone();
    let cidex_str = cidex_dir.to_string_lossy().to_string();

    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let in_cidex = event
                    .paths
                    .iter()
                    .all(|p| p.to_string_lossy().contains(&cidex_str));
                if in_cidex {
                    return;
                }

                match event.kind {
                    notify::EventKind::Create(_)
                    | notify::EventKind::Modify(_)
                    | notify::EventKind::Remove(_) => {
                        dirty_clone.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        })?;

    watcher.watch(&root, RecursiveMode::Recursive)?;

    let mut last_rebuild = Instant::now();
    let debounce = Duration::from_secs(2);

    loop {
        if dirty.load(Ordering::Relaxed) && last_rebuild.elapsed() > debounce {
            dirty.store(false, Ordering::Relaxed);

            eprint!("reindexing... ");
            match index::build(&root, true) {
                Ok(stats) => {
                    if stats.ngram_count > 0 {
                        eprintln!(
                            "done ({} files, {} n-grams, {:.2}s)",
                            stats.file_count, stats.ngram_count, stats.build_secs
                        );
                    } else {
                        eprintln!("up to date ({:.2}s)", stats.build_secs);
                    }
                }
                Err(e) => eprintln!("error: {}", e),
            }
            last_rebuild = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}
