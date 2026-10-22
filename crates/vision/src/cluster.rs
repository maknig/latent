use crate::VisionError;
use hdbscan::{Hdbscan, HdbscanHyperParams};

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Indices of `items` ordered by decreasing cosine similarity to `query` (at most `n`).
pub fn most_similar(query: &[f32], items: &[Vec<f32>], n: usize) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = items
        .iter()
        .enumerate()
        .map(|(i, v)| (i, cosine(query, v)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(n);
    scored
}

/// HDBSCAN over L2-normalised vectors. Returns one label per input: `Some(cluster)` or `None` for noise.
pub fn cluster(
    vectors: &[Vec<f32>],
    min_cluster_size: usize,
) -> Result<Vec<Option<usize>>, VisionError> {
    if vectors.len() < min_cluster_size.max(2) {
        return Ok(vec![None; vectors.len()]);
    }
    let data: Vec<Vec<f32>> = vectors
        .iter()
        .map(|v| {
            let n = v
                .iter()
                .map(|x| x * x)
                .sum::<f32>()
                .sqrt()
                .max(f32::EPSILON);
            v.iter().map(|x| x / n).collect()
        })
        .collect();
    let params = HdbscanHyperParams::builder()
        .min_cluster_size(min_cluster_size)
        .build();
    let labels = Hdbscan::new(&data, params)
        .cluster()
        .map_err(|e| VisionError::Cluster(format!("{e:?}")))?;
    Ok(labels
        .into_iter()
        .map(|l| usize::try_from(l).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn similar_orders_by_score() {
        let items = vec![vec![0.0, 1.0], vec![1.0, 0.1], vec![1.0, 0.0]];
        let r = most_similar(&[1.0, 0.0], &items, 2);
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![2, 1]);
    }

    #[test]
    fn two_blobs_two_clusters() {
        let mut v = Vec::new();
        for i in 0..10 {
            v.push(vec![1.0, 0.01 * i as f32]);
            v.push(vec![0.01 * i as f32, 1.0]);
        }
        let labels = cluster(&v, 3).unwrap();
        let a = labels[0];
        let b = labels[1];
        assert!(a.is_some() && b.is_some() && a != b);
        assert!(labels.iter().step_by(2).all(|l| *l == a));
        assert!(labels.iter().skip(1).step_by(2).all(|l| *l == b));
    }
}
