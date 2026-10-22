//! "Find similar images": embeds every archive image with the ONNX model in `<archive>/models/`
//! on a background thread (embeddings are cached in `filmarchiv.vision.json`) and lists the closest matches.
use crate::images::Thumbs;
use eframe::egui;
use fa_vision::{EmbeddingCache, Model, OnnxModel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

const TOP: usize = 12;

enum Msg {
    Progress(usize, usize),
    Done(String, Vec<(String, f32)>),
    Error(String),
}

pub struct Similar {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// (done, total) while a job runs.
    pub progress: Option<(usize, usize)>,
    /// Query image and its matches, shown in a window.
    result: Option<(String, Vec<(String, f32)>)>,
    pub error: Option<String>,
}

impl Default for Similar {
    fn default() -> Self {
        let (tx, rx) = channel();
        Similar { tx, rx, progress: None, result: None, error: None }
    }
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// The first `*.onnx` in `dir` that has a `.json` manifest beside it.
fn find_model(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "onnx") && p.with_extension("json").exists())
        .collect();
    found.sort();
    found.into_iter().next()
}

fn job(base: &Path, all: &[String], query: &str, tx: &Sender<Msg>, ctx: &egui::Context) -> Result<Vec<(String, f32)>, String> {
    let dir = base.join("models");
    let onnx = find_model(&dir).ok_or_else(|| format!("No model: put <name>.onnx and <name>.json into {}", dir.display()))?;
    let model = OnnxModel::load(&onnx).map_err(|e| e.to_string())?;
    let cache_path = base.join("filmarchiv.vision.json");
    let mut cache = EmbeddingCache::load(&cache_path);
    let mut paths: Vec<&String> = all.iter().filter(|p| p.as_str() != query).collect();
    paths.insert(0, all.iter().find(|p| p.as_str() == query).ok_or("image not in archive")?);
    let total = paths.len();
    let mut vectors: Vec<(String, Vec<f32>)> = Vec::with_capacity(total);
    for (n, rel) in paths.into_iter().enumerate() {
        let full = base.join(rel);
        let mt = mtime(&full);
        let v = match cache.get(model.id(), rel, mt) {
            Some(v) => Some(v.to_vec()),
            None => match image::open(&full).map_err(|e| e.to_string()).and_then(|im| model.run(&im).map_err(|e| e.to_string())) {
                Ok(v) => {
                    cache.insert(model.id(), rel, mt, v.clone());
                    Some(v)
                }
                // Unreadable images are skipped, not fatal.
                Err(_) if n > 0 => None,
                Err(e) => return Err(format!("{rel}: {e}")),
            },
        };
        if let Some(v) = v {
            vectors.push((rel.clone(), v));
        }
        if n % 20 == 0 {
            let _ = tx.send(Msg::Progress(n, total));
            ctx.request_repaint();
        }
    }
    cache.save(&cache_path).map_err(|e| e.to_string())?;
    let (query_vec, rest) = vectors.split_first().ok_or("no images")?;
    let items: Vec<Vec<f32>> = rest.iter().map(|(_, v)| v.clone()).collect();
    Ok(fa_vision::most_similar(&query_vec.1, &items, TOP)
        .into_iter()
        .map(|(i, s)| (rest[i].0.clone(), s))
        .collect())
}

impl Similar {
    pub fn start(&mut self, ctx: &egui::Context, base: PathBuf, all: Vec<String>, query: String) {
        if self.progress.is_some() {
            return;
        }
        self.progress = Some((0, all.len()));
        self.error = None;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let msg = match job(&base, &all, &query, &tx, &ctx) {
                Ok(r) => Msg::Done(query, r),
                Err(e) => Msg::Error(e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Applies finished work and draws the result window.
    pub fn show(&mut self, ctx: &egui::Context, thumbs: &mut Thumbs, base: &Path) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress(d, t) => self.progress = Some((d, t)),
                Msg::Done(q, r) => {
                    self.progress = None;
                    self.result = Some((q, r));
                }
                Msg::Error(e) => {
                    self.progress = None;
                    self.error = Some(e);
                }
            }
        }
        let mut open = true;
        if let Some((query, matches)) = &self.result {
            egui::Window::new("Similar images").open(&mut open).default_width(640.0).show(ctx, |ui| {
                ui.label(format!("Closest to {query}"));
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (p, score) in std::iter::once(&(query.clone(), 1.0)).chain(matches.iter()) {
                            ui.vertical(|ui| {
                                let (rect, resp) = ui.allocate_exact_size(egui::vec2(150.0, 110.0), egui::Sense::hover());
                                thumbs.paint_cropped(ui, ctx, &base.join(p), rect, 320);
                                resp.on_hover_text(p);
                                ui.weak(format!("{score:.2}"));
                            });
                        }
                    });
                });
            });
        }
        if !open {
            self.result = None;
        }
        if let Some(e) = self.error.clone() {
            egui::Window::new("Similar images").collapsible(false).show(ctx, |ui| {
                ui.label(e);
                if ui.button("Close").clicked() {
                    self.error = None;
                }
            });
        }
    }
}
