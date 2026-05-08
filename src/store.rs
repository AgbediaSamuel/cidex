use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use memmap2::Mmap;

use crate::varint;

const LOOKUP_RECORD_SIZE: usize = 24;

pub struct Index {
    lookup: Mmap,
    postings: Mmap,
    file_paths: Vec<PathBuf>,
    /// File IDs that weren't n-gram indexed (too large) and need brute-force search.
    unindexed: Vec<u32>,
}

#[derive(Debug)]
pub struct Meta {
    pub version: u32,
    pub file_count: u32,
    pub timestamp: u64,
    pub tree_size: u64,
}

impl Index {
    pub fn open(root: &Path) -> Result<Self> {
        let cidex = root.join(".cidex");
        if !cidex.exists() {
            bail!("no index found — run `cidex index` first");
        }

        let lookup_file =
            File::open(cidex.join("lookup.bin")).context("failed to open lookup.bin")?;
        let lookup = unsafe { Mmap::map(&lookup_file).context("failed to mmap lookup.bin")? };

        let postings_file =
            File::open(cidex.join("postings.bin")).context("failed to open postings.bin")?;
        let postings = unsafe { Mmap::map(&postings_file).context("failed to mmap postings.bin")? };

        let file_paths = read_file_paths(root, &cidex.join("files.bin"))?;
        let unindexed = read_unindexed(&cidex.join("unindexed.bin"))?;

        Ok(Index {
            lookup,
            postings,
            file_paths,
            unindexed,
        })
    }

    pub fn record_count(&self) -> usize {
        self.lookup.len() / LOOKUP_RECORD_SIZE
    }

    pub fn lookup(&self, hash: u64) -> Vec<u32> {
        let count = self.record_count();
        if count == 0 {
            return Vec::new();
        }

        let mut lo = 0usize;
        let mut hi = count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let record_hash = self.read_hash(mid);
            if record_hash < hash {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        if lo >= count || self.read_hash(lo) != hash {
            return Vec::new();
        }

        let offset = self.read_offset(lo);
        let length = self.read_length(lo);
        self.decode_posting_list(offset as usize, length as usize)
    }

    pub fn file_path(&self, file_id: u32) -> &Path {
        &self.file_paths[file_id as usize]
    }

    pub fn file_count(&self) -> u32 {
        self.file_paths.len() as u32
    }

    pub fn unindexed_file_ids(&self) -> &[u32] {
        &self.unindexed
    }

    #[inline(always)]
    fn read_hash(&self, index: usize) -> u64 {
        let base = index * LOOKUP_RECORD_SIZE;
        u64::from_le_bytes(self.lookup[base..base + 8].try_into().unwrap())
    }

    #[inline(always)]
    fn read_offset(&self, index: usize) -> u64 {
        let base = index * LOOKUP_RECORD_SIZE + 8;
        u64::from_le_bytes(self.lookup[base..base + 8].try_into().unwrap())
    }

    #[inline(always)]
    fn read_length(&self, index: usize) -> u32 {
        let base = index * LOOKUP_RECORD_SIZE + 16;
        u32::from_le_bytes(self.lookup[base..base + 4].try_into().unwrap())
    }

    fn decode_posting_list(&self, offset: usize, length: usize) -> Vec<u32> {
        let data = &self.postings[offset..offset + length];
        let mut pos = 0;
        let mut file_ids = Vec::new();
        let mut prev: u64 = 0;
        while pos < data.len() {
            let delta = varint::decode(data, &mut pos);
            prev += delta;
            file_ids.push(prev as u32);
        }
        file_ids
    }
}

pub fn read_meta(root: &Path) -> Result<Meta> {
    let path = root.join(".cidex/meta.bin");
    let mut f = File::open(&path).context("no index found")?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf)?;

    if &buf[0..4] != b"CIDX" {
        bail!("invalid index: bad magic");
    }

    Ok(Meta {
        version: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        file_count: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        timestamp: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
        tree_size: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
    })
}

fn read_unindexed(path: &Path) -> Result<Vec<u32>> {
    let data = fs::read(path).unwrap_or_default();
    let mut ids = Vec::with_capacity(data.len() / 4);
    let mut pos = 0;
    while pos + 4 <= data.len() {
        ids.push(u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()));
        pos += 4;
    }
    Ok(ids)
}

fn read_file_paths(root: &Path, path: &Path) -> Result<Vec<PathBuf>> {
    let data = fs::read(path).context("failed to read files.bin")?;
    let mut pos = 0;
    let mut paths = Vec::new();
    while pos + 2 <= data.len() {
        let len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + len > data.len() {
            break;
        }
        let rel =
            std::str::from_utf8(&data[pos..pos + len]).context("invalid utf-8 in file path")?;
        paths.push(root.join(rel));
        pos += len;
    }
    Ok(paths)
}
