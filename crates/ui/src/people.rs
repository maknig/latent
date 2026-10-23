//! "People": finds the faces in every archive image on a background thread (stored in `filmarchiv.faces.json`),
//! groups similar faces and lets you name each group. Named people are suggested by "Auto tags".
//! Needs `face-detect.onnx` and `face-embed.onnx` (+ `.json`) in `<archive>/models/`.
use crate::images::Thumbs;
use eframe::egui;
use fa_vision::{
    embed_faces, face_crop_path, find_kind, FaceDetector, FaceStore, OnnxModel, FACE_MATCH,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Smallest group of unnamed faces that is shown as a group.
const MIN_CLUSTER: usize = 3;
/// Faces shown per group.
const SHOWN: usize = 12;
const CROP: f32 = 64.0;
/// The store is written after this many newly scanned images.
const SAVE_EVERY: usize = 50;

#[derive(Clone)]
struct FaceRef {
    image: String,
    index: usize,
}

struct Group {
    person: Option<u32>,
    /// Name being edited.
    name: String,
    faces: Vec<FaceRef>,
}

enum Msg {
    Progress(usize, usize),
    /// Groups and the number of faces that belong to no group.
    Done(Vec<Group>, usize),
    Error(String),
}

pub struct People {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub open: bool,
    /// (done, total) while a scan runs.
    progress: Option<(usize, usize)>,
    cancel: Arc<AtomicBool>,
    groups: Vec<Group>,
    loose: usize,
    error: Option<String>,
}

impl Default for People {
    fn default() -> Self {
        let (tx, rx) = channel();
        People {
            tx,
            rx,
            open: false,
            progress: None,
            cancel: Default::default(),
            groups: vec![],
            loose: 0,
            error: None,
        }
    }
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

fn store_path(base: &Path) -> PathBuf {
    base.join("filmarchiv.faces.json")
}

/// Named people first, then clusters of unnamed faces, each biggest first. Also returns how many faces are in no group.
fn groups_of(store: &FaceStore) -> Result<(Vec<Group>, usize), String> {
    let mut named: BTreeMap<u32, Vec<FaceRef>> = BTreeMap::new();
    let mut unnamed: Vec<(FaceRef, &[f32])> = vec![];
    for (image, index, f) in store.all() {
        let r = FaceRef {
            image: image.to_string(),
            index,
        };
        match f.person {
            Some(p) => named.entry(p).or_default().push(r),
            None => unnamed.push((r, &f.embedding)),
        }
    }
    let mut groups: Vec<Group> = named
        .into_iter()
        .filter_map(|(p, faces)| {
            Some(Group {
                person: Some(p),
                name: store.person_name(p)?.to_string(),
                faces,
            })
        })
        .collect();
    groups.sort_by_key(|g| g.name.to_lowercase());
    let vectors: Vec<Vec<f32>> = unnamed.iter().map(|(_, v)| v.to_vec()).collect();
    let labels = fa_vision::cluster(&vectors, MIN_CLUSTER).map_err(|e| e.to_string())?;
    let mut clusters: BTreeMap<usize, Vec<FaceRef>> = BTreeMap::new();
    let mut loose = 0;
    for ((r, _), l) in unnamed.into_iter().zip(labels) {
        match l {
            Some(c) => clusters.entry(c).or_default().push(r),
            None => loose += 1,
        }
    }
    let mut clusters: Vec<Vec<FaceRef>> = clusters.into_values().collect();
    clusters.sort_by_key(|c| std::cmp::Reverse(c.len()));
    groups.extend(clusters.into_iter().map(|faces| Group {
        person: None,
        name: String::new(),
        faces,
    }));
    Ok((groups, loose))
}

fn job(
    base: &Path,
    all: &[String],
    cancel: &AtomicBool,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
) -> Result<(Vec<Group>, usize), String> {
    let models = base.join("models");
    let missing = |what: &str| {
        format!(
            "No {what} model: put face-{what}.onnx and face-{what}.json into {}",
            models.display()
        )
    };
    let det = find_kind(&models, Some("face-detect")).ok_or_else(|| missing("detect"))?;
    let emb = find_kind(&models, Some("face-embed")).ok_or_else(|| missing("embed"))?;
    let detector = FaceDetector::load(&det).map_err(|e| e.to_string())?;
    let embedder = OnnxModel::load(&emb).map_err(|e| e.to_string())?;
    let path = store_path(base);
    let mut store = FaceStore::load(&path);
    let total = all.len();
    let mut fresh = 0;
    for (n, rel) in all.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let full = base.join(rel);
        let mt = mtime(&full);
        if store.faces(rel, mt).is_none() {
            // Unreadable images are skipped, not fatal.
            if let Ok(img) = image::open(&full) {
                if let Ok(found) = embed_faces(&detector, &embedder, &img) {
                    for (i, (_, crop)) in found.iter().enumerate() {
                        let p = face_crop_path(base, rel, i);
                        if let Some(dir) = p.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = crop.save(&p);
                    }
                    store.set_faces(rel, mt, found.into_iter().map(|(f, _)| f).collect());
                    fresh += 1;
                    if fresh % SAVE_EVERY == 0 {
                        store.save(&path).map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        if n % 5 == 0 {
            let _ = tx.send(Msg::Progress(n, total));
            ctx.request_repaint();
        }
    }
    let known: HashSet<&str> = all.iter().map(String::as_str).collect();
    store.prune(&|p| known.contains(p));
    // New faces that clearly look like a named person join them.
    let open: Vec<(String, usize, Vec<f32>)> = store
        .all()
        .filter(|(_, _, f)| f.person.is_none())
        .map(|(p, i, f)| (p.to_string(), i, f.embedding.clone()))
        .collect();
    for (image, index, v) in open {
        if let Some((person, _)) = store.match_people(&v, "", FACE_MATCH).first() {
            store.assign(&image, index, Some(*person));
        }
    }
    store.save(&path).map_err(|e| e.to_string())?;
    groups_of(&store)
}

impl People {
    /// Opens the window and scans `all` (archive-relative image paths) in the background.
    pub fn start(&mut self, ctx: &egui::Context, base: PathBuf, all: Vec<String>) {
        self.open = true;
        if self.progress.is_some() {
            return;
        }
        self.progress = Some((0, all.len()));
        self.error = None;
        self.cancel.store(false, Ordering::Relaxed);
        let (tx, ctx, cancel) = (self.tx.clone(), ctx.clone(), self.cancel.clone());
        std::thread::spawn(move || {
            let msg = match job(&base, &all, &cancel, &tx, &ctx) {
                Ok((g, loose)) => Msg::Done(g, loose),
                Err(e) => Msg::Error(e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Names group `gi` (or clears the name when empty); a group that gets the name of another one joins it.
    fn name_group(&mut self, base: &Path, gi: usize) -> Result<(), String> {
        let path = store_path(base);
        let mut store = FaceStore::load(&path);
        let name = self.groups[gi].name.trim().to_string();
        let person = (!name.is_empty()).then(|| store.person_id(&name));
        for f in &self.groups[gi].faces {
            store.assign(&f.image, f.index, person);
        }
        store.prune(&|_| true);
        store.save(&path).map_err(|e| e.to_string())?;
        self.groups[gi].person = person;
        if let Some(p) = person {
            if let Some(other) = self
                .groups
                .iter()
                .position(|g| g.person == Some(p))
                .filter(|&o| o != gi)
            {
                let mut g = self.groups.remove(gi);
                let target = if other > gi { other - 1 } else { other };
                self.groups[target].faces.append(&mut g.faces);
            }
        }
        Ok(())
    }

    /// Takes one face out of a named group.
    fn remove_face(&mut self, base: &Path, gi: usize, fi: usize) -> Result<(), String> {
        let path = store_path(base);
        let mut store = FaceStore::load(&path);
        let f = self.groups[gi].faces.remove(fi);
        store.assign(&f.image, f.index, None);
        store.prune(&|_| true);
        store.save(&path).map_err(|e| e.to_string())?;
        if self.groups[gi].faces.is_empty() {
            self.groups.remove(gi);
        }
        self.loose += 1;
        Ok(())
    }

    /// Applies finished work and draws the window. Returns (rescan wanted, image path clicked).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        thumbs: &mut Thumbs,
        base: &Path,
    ) -> (bool, Option<String>) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress(d, t) => self.progress = Some((d, t)),
                Msg::Done(g, loose) => {
                    self.progress = None;
                    self.groups = g;
                    self.loose = loose;
                }
                Msg::Error(e) => {
                    self.progress = None;
                    self.error = Some(e);
                }
            }
        }
        let (mut rescan, mut clicked) = (false, None);
        if !self.open {
            return (rescan, clicked);
        }
        let (mut open, mut name, mut remove) = (true, None, None);
        egui::Window::new("People")
            .open(&mut open)
            .default_width(6.0 * (CROP + 8.0) + 80.0)
            .default_height(480.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| match self.progress {
                    Some((d, t)) => {
                        ui.add(
                            egui::ProgressBar::new(d as f32 / t.max(1) as f32)
                                .desired_width(200.0)
                                .text(format!("{d} / {t}")),
                        );
                        if ui.button("Stop").clicked() {
                            self.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    None => {
                        if ui.button("Scan archive").clicked() {
                            rescan = true;
                        }
                        ui.weak(format!(
                            "{} group(s), {} face(s) without a group",
                            self.groups.len(),
                            self.loose
                        ));
                    }
                });
                if let Some(e) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (gi, g) in self.groups.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut g.name)
                                        .hint_text("Name")
                                        .desired_width(140.0),
                                );
                                let enter = resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if ui.button("Set name").clicked() || enter {
                                    name = Some(gi);
                                }
                                ui.weak(format!(
                                    "{} face(s){}",
                                    g.faces.len(),
                                    if g.person.is_some() { "" } else { " (unnamed)" }
                                ));
                            });
                            ui.horizontal_wrapped(|ui| {
                                for (fi, f) in g.faces.iter().take(SHOWN).enumerate() {
                                    let (rect, resp) = ui.allocate_exact_size(
                                        egui::vec2(CROP, CROP),
                                        egui::Sense::click(),
                                    );
                                    if ui.is_rect_visible(rect) {
                                        thumbs.paint(
                                            ui,
                                            ctx,
                                            &face_crop_path(base, &f.image, f.index),
                                            rect,
                                            112,
                                        );
                                    }
                                    let resp = resp
                                        .on_hover_text(&f.image)
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    if resp.clicked() {
                                        clicked = Some(f.image.clone());
                                    }
                                    if g.person.is_some() {
                                        resp.context_menu(|ui| {
                                            if ui.button("Not this person").clicked() {
                                                remove = Some((gi, fi));
                                                ui.close_menu();
                                            }
                                        });
                                    }
                                }
                                if g.faces.len() > SHOWN {
                                    ui.weak(format!("+{}", g.faces.len() - SHOWN));
                                }
                            });
                            ui.separator();
                        }
                        if self.groups.is_empty() && self.progress.is_none() && self.error.is_none()
                        {
                            ui.weak("No groups yet. Scan the archive to find faces.");
                        }
                    });
            });
        // Only one change per frame: indices into `groups` are stale after the first one.
        let result = match (name, remove) {
            (Some(gi), _) => self.name_group(base, gi),
            (None, Some((gi, fi))) => self.remove_face(base, gi, fi),
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.error = Some(e);
        }
        self.open = open;
        (rescan, clicked)
    }
}
