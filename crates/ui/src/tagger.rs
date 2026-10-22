//! "Auto tags": suggests tags for an image from two sources, merged:
//! - its most similar tagged images (embedding model, e.g. DINOv2), and
//! - CLIP zero-shot scoring of the tags already used in the archive (`clip-image` + `clip-text` in `models/`).
//!
//! Embeddings are cached in `filmarchiv.vision.json`. Runs on a background thread.
use eframe::egui;
use fa_vision::{EmbeddingCache, Model, OnnxModel, TextModel};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

const NEIGHBOURS: usize = 10;
/// CLIP's logit scale.
const CLIP_SCALE: f32 = 100.0;

enum Msg {
    Done(String, Vec<(String, f32)>),
    Error(String),
}

pub struct Tagger {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub running: bool,
    /// Image the suggestions belong to, with (tag, score).
    pub result: Option<(String, Vec<(String, f32)>)>,
    pub error: Option<String>,
}

impl Default for Tagger {
    fn default() -> Self {
        let (tx, rx) = channel();
        Tagger { tx, rx, running: false, result: None, error: None }
    }
}

fn embedding(model: &OnnxModel, cache: &mut EmbeddingCache, base: &Path, rel: &str) -> Result<Vec<f32>, String> {
    let full = base.join(rel);
    let mtime = std::fs::metadata(&full).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    if let Some(v) = cache.get(model.id(), rel, mtime) {
        return Ok(v.to_vec());
    }
    let v = image::open(&full).map_err(|e| format!("{rel}: {e}")).and_then(|im| model.run(&im).map_err(|e| e.to_string()))?;
    cache.insert(model.id(), rel, mtime, v.clone());
    Ok(v)
}

/// Model files in `dir` of the given `kind` (`None` = plain embedding model).
fn find_kind(dir: &Path, kind: Option<&str>) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "onnx") && p.with_extension("json").exists() && fa_vision::kind_of(p).as_deref() == kind)
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Optional `tag_prompts.json`: tag -> English prompt for CLIP (tags are mostly German).
fn aliases(base: &Path) -> HashMap<String, String> {
    std::fs::read(base.join("tag_prompts.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn neighbours(models: &Path, base: &Path, cache: &mut EmbeddingCache, query: &str, tagged: &[(String, Vec<String>)]) -> Result<Vec<(String, f32)>, String> {
    let Some(onnx) = find_kind(models, None) else { return Ok(vec![]) };
    let model = OnnxModel::load(&onnx).map_err(|e| e.to_string())?;
    let q = embedding(&model, cache, base, query)?;
    let mut vectors = vec![];
    for (rel, tags) in tagged.iter().filter(|(rel, _)| rel != query) {
        // Unreadable images are skipped, not fatal.
        if let Ok(v) = embedding(&model, cache, base, rel) {
            vectors.push((v, tags.clone()));
        }
    }
    Ok(fa_vision::suggest_from_neighbours(&q, &vectors, NEIGHBOURS))
}

fn clip(models: &Path, base: &Path, cache: &mut EmbeddingCache, query: &str, vocabulary: &[String]) -> Result<Vec<(String, f32)>, String> {
    let (Some(img), Some(txt)) = (find_kind(models, Some("clip-image")), find_kind(models, Some("clip-text"))) else { return Ok(vec![]) };
    if vocabulary.is_empty() {
        return Ok(vec![]);
    }
    let image_model = OnnxModel::load(&img).map_err(|e| e.to_string())?;
    let text_model = TextModel::load(&txt).map_err(|e| e.to_string())?;
    let q = embedding(&image_model, cache, base, query)?;
    let aliases = aliases(base);
    let mut texts = vec![];
    for tag in vocabulary {
        let prompt = format!("a photo of {}", aliases.get(tag).map_or(tag.as_str(), String::as_str));
        // Text embeddings are cached by prompt (mtime is unused here).
        let v = match cache.get(text_model.id(), &prompt, 0) {
            Some(v) => v.to_vec(),
            None => {
                let v = text_model.embed(&prompt).map_err(|e| e.to_string())?;
                cache.insert(text_model.id(), &prompt, 0, v.clone());
                v
            }
        };
        texts.push((tag.clone(), v));
    }
    Ok(fa_vision::score_tags(&q, &texts, CLIP_SCALE))
}

/// Merges both sources by tag (case-insensitive): the better score wins, tags found by both get a bonus.
fn merge(a: Vec<(String, f32)>, b: Vec<(String, f32)>) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = a;
    for (tag, s) in b {
        match out.iter_mut().find(|(t, _)| t.to_lowercase() == tag.to_lowercase()) {
            Some(e) => e.1 = (e.1.max(s) + 0.1).min(1.0),
            None => out.push((tag, s)),
        }
    }
    out.sort_by(|x, y| y.1.total_cmp(&x.1));
    out
}

fn job(base: &Path, query: &str, tagged: &[(String, Vec<String>)]) -> Result<Vec<(String, f32)>, String> {
    let models = base.join("models");
    let cache_path = base.join("filmarchiv.vision.json");
    let mut cache = EmbeddingCache::load(&cache_path);
    let mut vocabulary: Vec<String> = vec![];
    for tag in tagged.iter().flat_map(|(_, t)| t) {
        if !vocabulary.iter().any(|v| v.to_lowercase() == tag.to_lowercase()) {
            vocabulary.push(tag.clone());
        }
    }
    let near = neighbours(&models, base, &mut cache, query, tagged)?;
    let by_clip = clip(&models, base, &mut cache, query, &vocabulary)?;
    cache.save(&cache_path).map_err(|e| e.to_string())?;
    if near.is_empty() && by_clip.is_empty() && find_kind(&models, None).is_none() && find_kind(&models, Some("clip-image")).is_none() {
        return Err(format!("No model in {}", models.display()));
    }
    Ok(merge(near, by_clip))
}

impl Tagger {
    pub fn start(&mut self, ctx: &egui::Context, base: PathBuf, query: String, tagged: Vec<(String, Vec<String>)>) {
        if self.running {
            return;
        }
        self.running = true;
        self.error = None;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let msg = match job(&base, &query, &tagged) {
                Ok(r) => Msg::Done(query, r),
                Err(e) => Msg::Error(e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            self.running = false;
            match m {
                Msg::Done(q, r) => self.result = Some((q, r)),
                Msg::Error(e) => self.error = Some(e),
            }
        }
    }
}
