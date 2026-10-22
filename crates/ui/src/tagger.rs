//! "Auto tags": suggests tags for an image from its most similar tagged images (DINOv2 embeddings,
//! cached in `filmarchiv.vision.json`). Runs on a background thread.
use eframe::egui;
use fa_vision::{EmbeddingCache, Model, OnnxModel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

const NEIGHBOURS: usize = 10;

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

fn job(base: &Path, query: &str, tagged: &[(String, Vec<String>)]) -> Result<Vec<(String, f32)>, String> {
    let onnx = crate::similar::find_model(&base.join("models")).ok_or("No model in the models folder")?;
    let model = OnnxModel::load(&onnx).map_err(|e| e.to_string())?;
    let cache_path = base.join("filmarchiv.vision.json");
    let mut cache = EmbeddingCache::load(&cache_path);
    let q = embedding(&model, &mut cache, base, query)?;
    let mut vectors = vec![];
    for (rel, tags) in tagged.iter().filter(|(rel, _)| rel != query) {
        // Unreadable images are skipped, not fatal.
        if let Ok(v) = embedding(&model, &mut cache, base, rel) {
            vectors.push((v, tags.clone()));
        }
    }
    cache.save(&cache_path).map_err(|e| e.to_string())?;
    Ok(fa_vision::suggest_from_neighbours(&q, &vectors, NEIGHBOURS))
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
