use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::ngram;
use crate::varint;

#[derive(Debug)]
pub struct IndexStats {
    pub file_count: u32,
    pub ngram_count: u64,
    pub build_secs: f64,
    pub postings_bytes: u64,
    pub lookup_bytes: u64,
}

const CIDEX_DIR: &str = ".cidex";
const META_MAGIC: &[u8; 4] = b"CIDX";
const META_VERSION: u32 = 1;
const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
const MAX_NGRAM_FILE_SIZE: u64 = 10 * 1024 * 1024;
const NGRAM_CHUNK_SIZE: usize = 10_000;

pub fn cidex_dir(root: &Path) -> PathBuf {
    root.join(CIDEX_DIR)
}

/// Per-file metadata gathered during the walk. Used to avoid re-stat'ing
/// every file later for fingerprinting and tree-size accounting.
struct FileEntry {
    path: PathBuf,
    mtime: u64,
    size: u64,
}

pub fn build(root: &Path, force: bool) -> Result<IndexStats> {
    let start = std::time::Instant::now();
    let cidex = cidex_dir(root);

    if force && cidex.exists() {
        fs::remove_dir_all(&cidex).context("failed to remove existing index")?;
    }
    fs::create_dir_all(&cidex).context("failed to create .cidex directory")?;

    let entries = walk_files(root);
    let fingerprint = compute_fingerprint(root, &entries);
    let fingerprint_path = cidex.join("fingerprint.bin");

    if !force && index_is_fresh(&fingerprint_path, &fingerprint) {
        let meta = crate::store::read_meta(root).ok();
        return Ok(IndexStats {
            file_count: meta.as_ref().map(|m| m.file_count).unwrap_or(0),
            ngram_count: 0,
            build_secs: start.elapsed().as_secs_f64(),
            postings_bytes: 0,
            lookup_bytes: 0,
        });
    }

    let file_count = entries.len() as u32;
    write_files_bin(&cidex, root, &entries)?;

    let pairs_path = cidex.join("pairs.tmp");
    let unindexed = extract_pairs_to_temp(&entries, &pairs_path)?;

    let (postings_bytes, lookup_bytes, ngram_count) = build_postings(&cidex, &pairs_path)?;

    write_meta(&cidex, file_count, &entries)?;
    write_unindexed(&cidex, unindexed)?;
    let _ = fs::remove_file(&pairs_path);

    // Write the fingerprint last so partial builds don't get mistaken
    // for fresh ones on the next run.
    let _ = fs::write(&fingerprint_path, &fingerprint);

    Ok(IndexStats {
        file_count,
        ngram_count,
        build_secs: start.elapsed().as_secs_f64(),
        postings_bytes,
        lookup_bytes,
    })
}

fn walk_files(root: &Path) -> Vec<FileEntry> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        // Apply .gitignore rules even when there's no .git directory.
        // Without this, target/ and similar build dirs leak in for non-git repos.
        .require_git(false)
        // Honor `.cidexignore` files (same syntax as .gitignore) for index-only excludes.
        .add_custom_ignore_filename(".cidexignore")
        .filter_entry(|entry| entry.file_name() != CIDEX_DIR)
        .build();

    let mut entries = Vec::new();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let size = meta.len();
        if size == 0 || size > MAX_FILE_SIZE {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        entries.push(FileEntry { path, mtime, size });
    }

    // Deterministic order for fingerprinting and stable file IDs.
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

fn index_is_fresh(fingerprint_path: &Path, fingerprint: &[u8]) -> bool {
    fs::read(fingerprint_path)
        .map(|stored| stored == fingerprint)
        .unwrap_or(false)
}

fn write_files_bin(cidex: &Path, root: &Path, entries: &[FileEntry]) -> Result<()> {
    let mut w = BufWriter::new(
        File::create(cidex.join("files.bin")).context("failed to create files.bin")?,
    );
    for entry in entries {
        let rel = entry.path.strip_prefix(root).unwrap_or(&entry.path);
        let bytes = rel.to_string_lossy();
        let bytes = bytes.as_bytes();
        let len = bytes.len() as u16;
        w.write_all(&len.to_le_bytes())?;
        w.write_all(bytes)?;
    }
    w.flush()?;
    Ok(())
}

/// Process files in chunks so the in-memory pair buffer stays bounded.
/// Each chunk is sorted before writing to the temp file; a final global
/// sort happens during postings construction.
///
/// Returns the set of file IDs whose contents weren't n-gram extracted
/// (too large or binary) — these get brute-force searched at query time.
fn extract_pairs_to_temp(entries: &[FileEntry], pairs_path: &Path) -> Result<Vec<u32>> {
    let unindexed = Mutex::new(Vec::<u32>::new());
    let mut pairs_writer =
        BufWriter::new(File::create(pairs_path).context("failed to create pairs temp file")?);

    for chunk_start in (0..entries.len()).step_by(NGRAM_CHUNK_SIZE) {
        let chunk_end = (chunk_start + NGRAM_CHUNK_SIZE).min(entries.len());
        let chunk = &entries[chunk_start..chunk_end];

        let mut chunk_pairs: Vec<(u64, u32)> = chunk
            .par_iter()
            .enumerate()
            .flat_map(|(i, entry)| {
                let file_id = (chunk_start + i) as u32;
                if entry.size > MAX_NGRAM_FILE_SIZE {
                    unindexed
                        .lock()
                        .expect("unindexed mutex poisoned")
                        .push(file_id);
                    return Vec::new();
                }
                let Ok(buf) = fs::read(&entry.path) else {
                    return Vec::new();
                };
                if is_binary(&buf) {
                    return Vec::new();
                }
                let mut hashes: Vec<u64> =
                    ngram::build_all(&buf).into_iter().map(|(h, _)| h).collect();
                hashes.sort_unstable();
                hashes.dedup();
                hashes.into_iter().map(|h| (h, file_id)).collect()
            })
            .collect();

        chunk_pairs.sort_unstable_by_key(|(h, _)| *h);
        for (hash, fid) in &chunk_pairs {
            pairs_writer.write_all(&hash.to_le_bytes())?;
            pairs_writer.write_all(&fid.to_le_bytes())?;
        }
    }
    pairs_writer.flush()?;

    let mut uids = unindexed.into_inner().expect("unindexed mutex poisoned");
    uids.sort_unstable();
    Ok(uids)
}

/// Final sort + group + delta+varint encoding for posting lists.
/// Returns (postings_bytes, lookup_bytes, ngram_count).
fn build_postings(cidex: &Path, pairs_path: &Path) -> Result<(u64, u64, u64)> {
    let pairs = read_pairs(pairs_path)?;

    let mut postings_writer = BufWriter::new(
        File::create(cidex.join("postings.bin")).context("failed to create postings.bin")?,
    );
    let mut lookup_entries: Vec<(u64, u64, u32)> = Vec::new();
    let mut offset: u64 = 0;
    let mut ngram_count: u64 = 0;

    let mut i = 0;
    while i < pairs.len() {
        let current_hash = pairs[i].0;
        let group_start = i;
        while i < pairs.len() && pairs[i].0 == current_hash {
            i += 1;
        }

        let mut file_ids: Vec<u32> = pairs[group_start..i].iter().map(|(_, fid)| *fid).collect();
        file_ids.sort_unstable();
        file_ids.dedup();

        let encoded = encode_posting_list(&file_ids);
        postings_writer.write_all(&encoded)?;
        lookup_entries.push((current_hash, offset, encoded.len() as u32));
        offset += encoded.len() as u64;
        ngram_count += 1;
    }
    postings_writer.flush()?;
    drop(pairs);

    let lookup_bytes = write_lookup_bin(cidex, &lookup_entries)?;
    Ok((offset, lookup_bytes, ngram_count))
}

fn read_pairs(pairs_path: &Path) -> Result<Vec<(u64, u32)>> {
    let raw = fs::read(pairs_path).context("failed to read pairs temp file")?;
    let pair_count = raw.len() / 12; // 8 bytes hash + 4 bytes file_id

    let mut pairs: Vec<(u64, u32)> = Vec::with_capacity(pair_count);
    let mut pos = 0;
    while pos + 12 <= raw.len() {
        let hash = u64::from_le_bytes(raw[pos..pos + 8].try_into().unwrap());
        let fid = u32::from_le_bytes(raw[pos + 8..pos + 12].try_into().unwrap());
        pairs.push((hash, fid));
        pos += 12;
    }
    pairs.sort_unstable();
    Ok(pairs)
}

fn encode_posting_list(file_ids: &[u32]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut prev: u64 = 0;
    for &fid in file_ids {
        let delta = fid as u64 - prev;
        varint::encode(delta, &mut buf);
        prev = fid as u64;
    }
    buf
}

fn write_lookup_bin(cidex: &Path, entries: &[(u64, u64, u32)]) -> Result<u64> {
    let mut w = BufWriter::new(
        File::create(cidex.join("lookup.bin")).context("failed to create lookup.bin")?,
    );
    for (hash, off, len) in entries {
        w.write_all(&hash.to_le_bytes())?;
        w.write_all(&off.to_le_bytes())?;
        w.write_all(&len.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?; // padding to 24-byte record
    }
    w.flush()?;
    Ok(entries.len() as u64 * 24)
}

fn write_meta(cidex: &Path, file_count: u32, entries: &[FileEntry]) -> Result<()> {
    let mut w =
        BufWriter::new(File::create(cidex.join("meta.bin")).context("failed to create meta.bin")?);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tree_size: u64 = entries.iter().map(|e| e.size).sum();

    w.write_all(META_MAGIC)?;
    w.write_all(&META_VERSION.to_le_bytes())?;
    w.write_all(&file_count.to_le_bytes())?;
    w.write_all(&0u32.to_le_bytes())?; // reserved
    w.write_all(&timestamp.to_le_bytes())?;
    w.write_all(&tree_size.to_le_bytes())?;
    w.flush()?;
    Ok(())
}

fn write_unindexed(cidex: &Path, file_ids: Vec<u32>) -> Result<()> {
    let mut w = BufWriter::new(
        File::create(cidex.join("unindexed.bin")).context("failed to create unindexed.bin")?,
    );
    for fid in &file_ids {
        w.write_all(&fid.to_le_bytes())?;
    }
    w.flush()?;
    Ok(())
}

fn compute_fingerprint(root: &Path, entries: &[FileEntry]) -> Vec<u8> {
    let mut data = Vec::new();
    for entry in entries {
        let rel = entry.path.strip_prefix(root).unwrap_or(&entry.path);
        data.extend_from_slice(rel.to_string_lossy().as_bytes());
        data.push(0);
        data.extend_from_slice(&entry.mtime.to_le_bytes());
        data.extend_from_slice(&entry.size.to_le_bytes());
    }
    // FNV-1a — change detection, not crypto.
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for &b in &data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash ^= entries.len() as u64;
    hash.to_le_bytes().to_vec()
}

fn is_binary(buf: &[u8]) -> bool {
    let check_len = buf.len().min(8192);
    buf[..check_len].contains(&0u8)
}
