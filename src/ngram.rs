use crate::freq::WEIGHT;

pub fn hash_ngram(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0;
    for &b in bytes {
        h = h.wrapping_mul(31).wrapping_add(b as u64);
    }
    h
}

fn weight(a: u8, b: u8) -> f32 {
    WEIGHT[a as usize][b as usize]
}

// Compute the weight at each position: w[i] = weight(byte[i], byte[i+1])
fn weight_sequence(content: &[u8]) -> Vec<f32> {
    if content.len() < 2 {
        return Vec::new();
    }
    (0..content.len() - 1)
        .map(|i| weight(content[i], content[i + 1]))
        .collect()
}

// Find all local maxima positions in the weight sequence.
// A position i is a local maximum if w[i] > w[i-1] and w[i] >= w[i+1]
// (strict left, non-strict right to break ties deterministically).
fn local_maxima(weights: &[f32]) -> Vec<usize> {
    if weights.is_empty() {
        return Vec::new();
    }
    let mut maxima = Vec::new();

    // First position is a boundary if it's >= the next
    if weights.len() == 1 || weights[0] >= weights[1] {
        maxima.push(0);
    }

    for i in 1..weights.len().saturating_sub(1) {
        if weights[i] > weights[i - 1] && weights[i] >= weights[i + 1] {
            maxima.push(i);
        }
    }

    // Last position is a boundary if it's > the previous
    if weights.len() > 1 && weights[weights.len() - 1] > weights[weights.len() - 2] {
        maxima.push(weights.len() - 1);
    }

    maxima
}

// Extract all sparse n-grams from content.
// Returns (hash, byte_position) for each n-gram.
pub fn build_all(content: &[u8]) -> Vec<(u64, usize)> {
    if content.len() < 3 {
        return Vec::new();
    }

    let weights = weight_sequence(content);
    let maxima = local_maxima(&weights);

    if maxima.len() < 2 {
        // Entire content is one n-gram
        return vec![(hash_ngram(content), 0)];
    }

    let mut ngrams = Vec::new();
    for window in maxima.windows(2) {
        let start = window[0];
        // +2 because weight[i] covers bytes[i..i+2], so the n-gram
        // spans from start to end+2 (exclusive)
        let end = (window[1] + 2).min(content.len());
        let gram = &content[start..end];
        ngrams.push((hash_ngram(gram), start));
    }

    ngrams
}

// Extract covering n-gram hashes for a query literal.
//
// Edge n-grams from build_all are context-dependent — the surrounding bytes
// change which positions become local maxima. We use only strictly interior
// maxima (positions where both neighbors are within the literal), which are
// stable regardless of what surrounds the literal in a document.
//
// If there are fewer than 2 interior maxima we can't form a stable n-gram,
// so we return empty and the caller falls back to scanning all files.
pub fn build_covering(literal: &[u8]) -> Vec<u64> {
    if literal.len() < 3 {
        return Vec::new();
    }

    let weights = weight_sequence(literal);
    if weights.is_empty() {
        return Vec::new();
    }

    // Find strictly interior maxima (not at position 0 or len-1 of the weight seq)
    let mut interior_maxima = Vec::new();
    for i in 1..weights.len().saturating_sub(1) {
        if weights[i] > weights[i - 1] && weights[i] >= weights[i + 1] {
            interior_maxima.push(i);
        }
    }

    if interior_maxima.len() >= 2 {
        // Form n-grams between consecutive interior maxima
        let mut hashes = Vec::new();
        for window in interior_maxima.windows(2) {
            let start = window[0];
            let end = (window[1] + 2).min(literal.len());
            hashes.push(hash_ngram(&literal[start..end]));
        }
        return hashes;
    }

    // Fewer than 2 interior maxima — can't form a stable n-gram.
    // Return empty to signal the caller should scan all files.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_all_empty() {
        assert!(build_all(b"").is_empty());
    }

    #[test]
    fn build_all_short() {
        assert!(build_all(b"ab").is_empty());
    }

    #[test]
    fn build_all_produces_ngrams() {
        let content = b"fn main() { println!(\"hello\"); }";
        let ngrams = build_all(content);
        assert!(!ngrams.is_empty());
    }

    #[test]
    fn build_covering_subset_of_build_all() {
        let content = b"some content with MAX_FILE_SIZE in it";
        let literal = b"MAX_FILE_SIZE";

        let all_hashes: std::collections::HashSet<u64> =
            build_all(content).into_iter().map(|(h, _)| h).collect();
        let covering = build_covering(literal);

        for h in &covering {
            assert!(
                all_hashes.contains(h),
                "covering hash {} not found in build_all output",
                h
            );
        }
    }

    #[test]
    fn build_covering_deterministic() {
        let literal = b"MAX_FILE_SIZE";
        let a = build_covering(literal);
        let b = build_covering(literal);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_ngram_deterministic() {
        assert_eq!(hash_ngram(b"abc"), hash_ngram(b"abc"));
        assert_ne!(hash_ngram(b"abc"), hash_ngram(b"abd"));
    }

    #[test]
    fn local_maxima_basic() {
        let weights = vec![1.0, 3.0, 2.0, 5.0, 1.0];
        let maxima = local_maxima(&weights);
        assert!(maxima.contains(&1));
        assert!(maxima.contains(&3));
    }

    #[test]
    fn covering_in_context() {
        let file_content = b"    let MAX_FILE_SIZE: usize = 1024;";
        let literal = b"MAX_FILE_SIZE";

        let covering = build_covering(literal);
        assert!(!covering.is_empty(), "long literal should produce covering hashes");

        let file_hashes: std::collections::HashSet<u64> =
            build_all(file_content).into_iter().map(|(h, _)| h).collect();

        for h in &covering {
            assert!(file_hashes.contains(h), "covering hash not in file n-grams");
        }
    }

    #[test]
    fn short_literal_returns_empty_covering() {
        // 'return' is too short for stable interior maxima
        let covering = build_covering(b"return");
        assert!(covering.is_empty());
    }
}
