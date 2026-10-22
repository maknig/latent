use crate::cosine;

/// Tags in a comma/newline separated cell, trimmed, without empty entries.
pub fn split_tags(s: &str) -> Vec<String> {
    s.split([',', '\n']).map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect()
}

/// Tags for `query` voted by its `k` most similar tagged images: a tag's score is the share of the
/// neighbours' summed similarity that carries it (0..1). Sorted by score, best first.
pub fn suggest_from_neighbours(query: &[f32], tagged: &[(Vec<f32>, Vec<String>)], k: usize) -> Vec<(String, f32)> {
    let mut near: Vec<(f32, &Vec<String>)> = tagged.iter().map(|(v, t)| (cosine(query, v).max(0.0), t)).collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0));
    near.truncate(k);
    let total: f32 = near.iter().map(|n| n.0).sum();
    if total <= 0.0 {
        return vec![];
    }
    // Tags are grouped case-insensitively; the first spelling seen (best neighbour) is kept.
    let mut votes: Vec<(String, f32)> = vec![];
    for (sim, tags) in near {
        for t in tags {
            match votes.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(t) || n.to_lowercase() == t.to_lowercase()) {
                Some(v) => v.1 += sim,
                None => votes.push((t.clone(), sim)),
            }
        }
    }
    let mut out: Vec<(String, f32)> = votes.into_iter().map(|(t, s)| (t, s / total)).collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn splits_and_trims() {
        assert_eq!(split_tags("Lara, Montsant,\nSpanien,"), tags(&["Lara", "Montsant", "Spanien"]));
        assert!(split_tags(" , ").is_empty());
    }

    #[test]
    fn close_neighbours_decide() {
        let tagged = vec![
            (vec![1.0, 0.0], tags(&["Berge", "Schnee"])),
            (vec![0.9, 0.1], tags(&["berge", "Winter"])),
            (vec![0.0, 1.0], tags(&["Portrait"])),
        ];
        let r = suggest_from_neighbours(&[1.0, 0.0], &tagged, 2);
        assert_eq!(r[0].0, "Berge");
        assert!((r[0].1 - 1.0).abs() < 1e-6);
        assert!(r.iter().all(|(t, _)| t != "Portrait"));
        assert!(r[1].1 < r[0].1);
    }

    #[test]
    fn nothing_tagged_nothing_suggested() {
        assert!(suggest_from_neighbours(&[1.0], &[], 5).is_empty());
    }
}
