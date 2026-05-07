use anyhow::Result;
use regex_syntax::hir::{Hir, HirKind};

use crate::ngram;
use crate::store::Index;

#[derive(Debug)]
pub enum QueryPlan {
    And(Vec<QueryPlan>),
    Or(Vec<QueryPlan>),
    Literal(Vec<u8>),
    Scan, // no usable literals — brute-force all files
}

// Extract literal fragments from a regex pattern.
// This is the simple version: walk the HIR tree, pull out Literal nodes,
// combine with And (concatenation) or Or (alternation).
pub fn extract_literals(pattern: &str) -> Result<QueryPlan> {
    let hir = regex_syntax::parse(pattern)?;
    let plan = hir_to_plan(&hir);
    Ok(simplify(plan))
}

fn hir_to_plan(hir: &Hir) -> QueryPlan {
    match hir.kind() {
        HirKind::Literal(lit) => {
            let bytes = lit.0.to_vec();
            if bytes.len() < 3 {
                // too short for n-gram extraction
                QueryPlan::Scan
            } else {
                QueryPlan::Literal(bytes)
            }
        }
        HirKind::Concat(subs) => {
            // Try to merge adjacent literals
            let mut parts: Vec<QueryPlan> = Vec::new();
            let mut current_literal: Vec<u8> = Vec::new();

            for sub in subs {
                if let HirKind::Literal(lit) = sub.kind() {
                    current_literal.extend_from_slice(&lit.0);
                } else {
                    if current_literal.len() >= 3 {
                        parts.push(QueryPlan::Literal(current_literal.clone()));
                    }
                    current_literal.clear();
                    let sub_plan = hir_to_plan(sub);
                    if !matches!(sub_plan, QueryPlan::Scan) {
                        parts.push(sub_plan);
                    }
                }
            }
            if current_literal.len() >= 3 {
                parts.push(QueryPlan::Literal(current_literal));
            }

            if parts.is_empty() {
                QueryPlan::Scan
            } else if parts.len() == 1 {
                parts.into_iter().next().unwrap()
            } else {
                QueryPlan::And(parts)
            }
        }
        HirKind::Alternation(subs) => {
            let plans: Vec<QueryPlan> = subs.iter().map(hir_to_plan).collect();
            if plans.iter().any(|p| matches!(p, QueryPlan::Scan)) {
                // If any branch is unsearchable, the whole alternation is
                QueryPlan::Scan
            } else if plans.len() == 1 {
                plans.into_iter().next().unwrap()
            } else {
                QueryPlan::Or(plans)
            }
        }
        HirKind::Capture(cap) => hir_to_plan(&cap.sub),
        HirKind::Repetition(_) | HirKind::Class(_) | HirKind::Look(_) | HirKind::Empty => {
            QueryPlan::Scan
        }
    }
}

fn simplify(plan: QueryPlan) -> QueryPlan {
    match plan {
        QueryPlan::And(parts) => {
            let parts: Vec<_> = parts.into_iter().map(simplify).collect();
            let parts: Vec<_> = parts
                .into_iter()
                .filter(|p| !matches!(p, QueryPlan::Scan))
                .collect();
            if parts.is_empty() {
                QueryPlan::Scan
            } else if parts.len() == 1 {
                parts.into_iter().next().unwrap()
            } else {
                QueryPlan::And(parts)
            }
        }
        QueryPlan::Or(parts) => {
            let parts: Vec<_> = parts.into_iter().map(simplify).collect();
            if parts.iter().any(|p| matches!(p, QueryPlan::Scan)) {
                QueryPlan::Scan
            } else if parts.len() == 1 {
                parts.into_iter().next().unwrap()
            } else {
                QueryPlan::Or(parts)
            }
        }
        other => other,
    }
}

// Execute a query plan against the index.
// Returns sorted, deduplicated file IDs.
// Always includes unindexed files (too large for n-gram extraction)
// so they get brute-force searched.
pub fn execute(plan: &QueryPlan, index: &Index) -> Vec<u32> {
    let mut result = execute_inner(plan, index);
    // Merge in unindexed file IDs
    let unindexed = index.unindexed_file_ids();
    if !unindexed.is_empty() {
        result = union(&result, unindexed);
    }
    result
}

fn execute_inner(plan: &QueryPlan, index: &Index) -> Vec<u32> {
    match plan {
        QueryPlan::Literal(bytes) => {
            let hashes = ngram::build_covering(bytes);
            if hashes.is_empty() {
                return all_file_ids(index);
            }
            let mut result: Option<Vec<u32>> = None;
            for hash in hashes {
                let posting = index.lookup(hash);
                result = Some(match result {
                    None => posting,
                    Some(prev) => intersect(&prev, &posting),
                });
            }
            result.unwrap_or_default()
        }
        QueryPlan::And(parts) => {
            let mut result: Option<Vec<u32>> = None;
            for part in parts {
                let ids = execute_inner(part, index);
                result = Some(match result {
                    None => ids,
                    Some(prev) => intersect(&prev, &ids),
                });
            }
            result.unwrap_or_default()
        }
        QueryPlan::Or(parts) => {
            let mut result: Vec<u32> = Vec::new();
            for part in parts {
                let ids = execute_inner(part, index);
                result = union(&result, &ids);
            }
            result
        }
        QueryPlan::Scan => all_file_ids(index),
    }
}

fn all_file_ids(index: &Index) -> Vec<u32> {
    (0..index.file_count()).collect()
}

// Intersect two sorted vecs
fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            result.push(a[i]);
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    result
}

// Union two sorted vecs
fn union(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            result.push(a[i]);
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            result.push(a[i]);
            i += 1;
        } else {
            result.push(b[j]);
            j += 1;
        }
    }
    result.extend_from_slice(&a[i..]);
    result.extend_from_slice(&b[j..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_extraction() {
        let plan = extract_literals("hello").unwrap();
        assert!(matches!(plan, QueryPlan::Literal(ref b) if b == b"hello"));
    }

    #[test]
    fn alternation() {
        let plan = extract_literals("foo|bar").unwrap();
        assert!(matches!(plan, QueryPlan::Or(_)));
    }

    #[test]
    fn concat_with_wildcard() {
        let plan = extract_literals("foo.*bar").unwrap();
        assert!(matches!(plan, QueryPlan::And(_)));
        if let QueryPlan::And(parts) = plan {
            assert_eq!(parts.len(), 2);
        }
    }

    #[test]
    fn pure_wildcard() {
        let plan = extract_literals(".*").unwrap();
        assert!(matches!(plan, QueryPlan::Scan));
    }

    #[test]
    fn short_literal_becomes_scan() {
        let plan = extract_literals("ab").unwrap();
        assert!(matches!(plan, QueryPlan::Scan));
    }

    #[test]
    fn intersect_basic() {
        assert_eq!(intersect(&[1, 3, 5, 7], &[2, 3, 5, 8]), vec![3, 5]);
    }

    #[test]
    fn union_basic() {
        assert_eq!(union(&[1, 3, 5], &[2, 3, 6]), vec![1, 2, 3, 5, 6]);
    }

    #[test]
    fn intersect_with_empty() {
        assert_eq!(intersect(&[1, 2, 3], &[]), Vec::<u32>::new());
    }

    #[test]
    fn union_with_empty() {
        assert_eq!(union(&[1, 2, 3], &[]), vec![1, 2, 3]);
    }
}
