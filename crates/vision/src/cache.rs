use crate::VisionError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct Entry {
    mtime: u64,
    vector: Vec<f32>,
}

/// Embeddings keyed by (model id, archive-relative image path), invalidated by file mtime.
/// Stored as a sidecar JSON file next to the archive data.
#[derive(Default, Serialize, Deserialize)]
pub struct EmbeddingCache {
    entries: HashMap<String, HashMap<String, Entry>>,
}

impl EmbeddingCache {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, model: &str, image: &str, mtime: u64) -> Option<&[f32]> {
        let e = self.entries.get(model)?.get(image)?;
        (e.mtime == mtime).then_some(e.vector.as_slice())
    }

    pub fn insert(&mut self, model: &str, image: &str, mtime: u64, vector: Vec<f32>) {
        self.entries
            .entry(model.to_string())
            .or_default()
            .insert(image.to_string(), Entry { mtime, vector });
    }

    /// Atomic save (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), VisionError> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_mtime_invalidation() {
        let mut c = EmbeddingCache::default();
        c.insert("m", "a.png", 5, vec![1.0, 2.0]);
        assert_eq!(c.get("m", "a.png", 5), Some(&[1.0, 2.0][..]));
        assert!(c.get("m", "a.png", 6).is_none());
        assert!(c.get("other", "a.png", 5).is_none());
        let p =
            std::env::temp_dir().join(format!("latent-vision-cache-{}.json", std::process::id()));
        c.save(&p).unwrap();
        let l = EmbeddingCache::load(&p);
        assert_eq!(l.get("m", "a.png", 5), Some(&[1.0, 2.0][..]));
        let _ = std::fs::remove_file(p);
    }
}
