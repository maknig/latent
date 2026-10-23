//! "People": finds the faces in every archive image on a background thread (stored in `filmarchiv.faces.json`),
//! groups similar faces and lets you name each group. Named people are suggested by "Auto tags".
//! Needs `face-detect.onnx` and `face-embed.onnx` (+ `.json`) in `<archive>/models/`.
use crate::images::Thumbs;
use eframe::egui;
use fa_vision::{
    embed_faces, face_crop_path, find_kind, save_crops, FaceDetector, FaceStore, OnnxModel,
    FACE_MATCH,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Smallest group of unnamed faces that is shown as a group.
const MIN_CLUSTER: usize = 3;
/// Faces shown per collapsed group.
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
    /// The faces that belong to no group (cannot be named as a whole).
    loose: bool,
    expanded: bool,
    /// Name being edited.
    name: String,
    faces: Vec<FaceRef>,
}

enum Msg {
    Progress(usize, usize),
    Done(Vec<Group>),
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

/// Named people first, then clusters of unnamed faces (biggest first), then the faces that are in no cluster.
fn groups_of(store: &FaceStore) -> Result<Vec<Group>, String> {
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
                loose: false,
                expanded: false,
                name: store.person_name(p)?.to_string(),
                faces,
            })
        })
        .collect();
    groups.sort_by_key(|g| g.name.to_lowercase());
    let vectors: Vec<Vec<f32>> = unnamed.iter().map(|(_, v)| v.to_vec()).collect();
    let labels = fa_vision::cluster(&vectors, MIN_CLUSTER).map_err(|e| e.to_string())?;
    let mut clusters: BTreeMap<usize, Vec<FaceRef>> = BTreeMap::new();
    let mut loose = vec![];
    for ((r, _), l) in unnamed.into_iter().zip(labels) {
        match l {
            Some(c) => clusters.entry(c).or_default().push(r),
            None => loose.push(r),
        }
    }
    let mut clusters: Vec<Vec<FaceRef>> = clusters.into_values().collect();
    clusters.sort_by_key(|c| std::cmp::Reverse(c.len()));
    groups.extend(clusters.into_iter().map(|faces| Group {
        person: None,
        loose: false,
        expanded: false,
        name: String::new(),
        faces,
    }));
    if !loose.is_empty() {
        groups.push(Group {
            person: None,
            loose: true,
            expanded: false,
            name: String::new(),
            faces: loose,
        });
    }
    Ok(groups)
}

fn job(
    base: &Path,
    all: &[String],
    cancel: &AtomicBool,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
) -> Result<Vec<Group>, String> {
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
                    store.set_faces(rel, mt, save_crops(base, rel, found));
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
                Ok(g) => Msg::Done(g),
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

    /// Moves one face to `person` (`None`: to the faces without a group) and saves it.
    fn move_face(
        &mut self,
        base: &Path,
        gi: usize,
        fi: usize,
        person: Option<u32>,
    ) -> Result<(), String> {
        let path = store_path(base);
        let mut store = FaceStore::load(&path);
        let f = self.groups[gi].faces.remove(fi);
        store.assign(&f.image, f.index, person);
        store.prune(&|_| true);
        store.save(&path).map_err(|e| e.to_string())?;
        if self.groups[gi].faces.is_empty() {
            self.groups.remove(gi);
        }
        let target = self
            .groups
            .iter()
            .position(|g| g.person == person && g.person.is_some() || person.is_none() && g.loose);
        match target {
            Some(t) => self.groups[t].faces.push(f),
            None => self.groups.push(Group {
                person,
                loose: person.is_none(),
                expanded: false,
                name: person
                    .and_then(|p| store.person_name(p))
                    .unwrap_or_default()
                    .to_string(),
                faces: vec![f],
            }),
        }
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
                Msg::Done(g) => {
                    self.progress = None;
                    self.groups = g;
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
        let (mut open, mut name, mut moved) = (true, None, None);
        let faces: usize = self.groups.iter().map(|g| g.faces.len()).sum();
        let loose: usize = self
            .groups
            .iter()
            .filter(|g| g.loose)
            .map(|g| g.faces.len())
            .sum();
        let named: Vec<(u32, String)> = self
            .groups
            .iter()
            .filter_map(|g| Some((g.person?, g.name.clone())))
            .collect();
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
                            "{faces} face(s), {} named, {loose} without a group",
                            named.len()
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
                                if g.loose {
                                    ui.strong("Without a group");
                                    ui.weak(format!(
                                        "{} face(s): right-click a face to name it",
                                        g.faces.len()
                                    ));
                                    return;
                                }
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
                                let shown = if g.expanded { g.faces.len() } else { SHOWN };
                                for (fi, f) in g.faces.iter().take(shown).enumerate() {
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
                                    if g.person.is_some() || g.loose {
                                        resp.context_menu(|ui| {
                                            if g.person.is_some()
                                                && ui.button("Not this person").clicked()
                                            {
                                                moved = Some((gi, fi, None));
                                                ui.close_menu();
                                            }
                                            for (id, n) in &named {
                                                if g.person != Some(*id)
                                                    && ui.button(format!("This is {n}")).clicked()
                                                {
                                                    moved = Some((gi, fi, Some(*id)));
                                                    ui.close_menu();
                                                }
                                            }
                                        });
                                    }
                                }
                                if g.faces.len() > SHOWN {
                                    let label = if g.expanded {
                                        "Show fewer".to_string()
                                    } else {
                                        format!("Show all ({})", g.faces.len())
                                    };
                                    if ui.button(label).clicked() {
                                        g.expanded = !g.expanded;
                                    }
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
        let result = match (name, moved) {
            (Some(gi), _) => self.name_group(base, gi),
            (None, Some((gi, fi, person))) => self.move_face(base, gi, fi, person),
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.error = Some(e);
        }
        self.open = open;
        (rescan, clicked)
    }
}
