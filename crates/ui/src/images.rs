//! Background thumbnail loading and texture cache (native only).
//!
//! Thumbnails are decoded by a small worker pool and cached on disk below
//! `<archive>/.thumbs`, so the full-size originals are decoded only once.
//! Textures are evicted when unused.
use crate::histogram::{image_stats, ImageStats};
use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};


pub enum Thumb {
    Loading,
    Ready(egui::TextureHandle),
    Missing,
}

type Key = (PathBuf, u32);
type Payload = Option<(egui::ColorImage, Option<Box<ImageStats>>)>;

/// What a worker reports back for a job.
enum Outcome {
    Done(Payload),
    /// The image was no longer wanted when the job came up; nothing was decoded.
    Skipped,
}
type Loaded = (Key, Outcome);

/// Thumbnails up to this size are cached on disk; larger ones (preview) are not.
const DISK_CACHE_MAX: u32 = 512;
/// Entries at least this large are dropped as soon as they were not drawn last frame,
/// and at most one of them is uploaded to the GPU per frame.
const TRANSIENT_MIN: u32 = 1024;
const MAX_ENTRIES: usize = 300;
/// A queued job is dropped when its image was not requested for this many frames.
const STALE_FRAMES: u64 = 5;

struct Entry {
    thumb: Thumb,
    hist: Option<Box<ImageStats>>,
    last_used: u64,
    /// Frame in which the image was last requested; shared with the queued job.
    wanted: Arc<AtomicU64>,
}

struct Job {
    key: Key,
    cache_dir: Option<PathBuf>,
    wanted: Arc<AtomicU64>,
}

/// Job queue shared with the workers; interactive jobs (`high`) go before preloads (`low`).
#[derive(Default)]
struct Queue {
    jobs: Mutex<(Vec<Job>, Vec<Job>)>,
    signal: Condvar,
}

pub struct Thumbs {
    map: HashMap<Key, Entry>,
    frame: u64,
    now: Arc<AtomicU64>,
    cache_dir: Option<PathBuf>,
    queue: Option<Arc<Queue>>,
    pending: VecDeque<Loaded>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

impl Default for Thumbs {
    fn default() -> Self {
        let (tx, rx) = channel();
        Thumbs { map: HashMap::new(), frame: 0, now: Arc::default(), cache_dir: None, queue: None, pending: VecDeque::new(), tx, rx }
    }
}

/// Cache file for `path` at `size`; changes when the file's size or mtime change.
fn cache_file(dir: &Path, path: &Path, size: u32) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (path, meta.len(), mtime.as_nanos()).hash(&mut h);
    Some(dir.join(size.to_string()).join(format!("{:016x}.png", h.finish())))
}

/// Rotates the image file by `quarter_turns` clockwise (negative = counter-clockwise), keeping its format.
/// The result is written to a temp file and renamed over the original; JPEGs are re-encoded (quality 95, no EXIF).
pub fn rotate_file(path: &Path, quarter_turns: i32) -> Result<(), String> {
    use image::{codecs::jpeg::JpegEncoder, ImageFormat};
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let format = match ext.as_str() {
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "png" => ImageFormat::Png,
        "tif" | "tiff" => ImageFormat::Tiff,
        _ => return Err(format!("{}: unsupported format", path.display())),
    };
    let im = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let im = match quarter_turns.rem_euclid(4) {
        0 => return Ok(()),
        1 => im.rotate90(),
        2 => im.rotate180(),
        _ => im.rotate270(),
    };
    let tmp = path.with_extension(format!("{ext}.tmp"));
    let saved = if format == ImageFormat::Jpeg {
        std::fs::File::create(&tmp)
            .map_err(|e| e.to_string())
            .and_then(|f| im.to_rgb8().write_with_encoder(JpegEncoder::new_with_quality(std::io::BufWriter::new(f), 95)).map_err(|e| e.to_string()))
    } else {
        im.save_with_format(&tmp, format).map_err(|e| e.to_string())
    };
    saved.and_then(|()| std::fs::rename(&tmp, path).map_err(|e| e.to_string())).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

fn to_color_image(im: &image::RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([im.width() as usize, im.height() as usize], im.as_raw())
}

fn load(job: Job) -> Payload {
    let (path, size) = &job.key;
    let cached = if *size <= DISK_CACHE_MAX { job.cache_dir.as_deref().and_then(|d| cache_file(d, path, *size)) } else { None };
    if let Some(c) = &cached {
        if let Ok(im) = image::open(c) {
            return Some((to_color_image(&im.to_rgba8()), None));
        }
    }
    let im = image::open(path).ok()?;
    let im = if im.width() > *size || im.height() > *size { im.resize(*size, *size, image::imageops::FilterType::Triangle) } else { im }.to_rgba8();
    if let Some(c) = cached {
        if let Some(dir) = c.parent() {
            let tmp = c.with_extension("tmp");
            // Best effort: a failed cache write only costs a re-decode later.
            if std::fs::create_dir_all(dir).is_ok() && im.save_with_format(&tmp, image::ImageFormat::Png).is_ok() {
                let _ = std::fs::rename(&tmp, &c);
            }
        }
    }
    let hist = (*size >= TRANSIENT_MIN).then(|| Box::new(image_stats(&im)));
    Some((to_color_image(&im), hist))
}

fn spawn_workers(tx: Sender<Loaded>, ctx: egui::Context, now: Arc<AtomicU64>) -> Arc<Queue> {
    let queue = Arc::new(Queue::default());
    // Decoding is heavy; leave most cores to the UI and the rest of the desktop.
    let n = std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).clamp(1, 2));
    for _ in 0..n {
        let (queue, tx, ctx, now) = (queue.clone(), tx.clone(), ctx.clone(), now.clone());
        std::thread::spawn(move || loop {
            let job = {
                let mut jobs = queue.jobs.lock().unwrap();
                loop {
                    // Newest interactive request first; preloads only when nothing else waits.
                    if let Some(j) = jobs.0.pop().or_else(|| jobs.1.pop()) {
                        break j;
                    }
                    jobs = queue.signal.wait(jobs).unwrap();
                }
            };
            let key = job.key.clone();
            let stale = now.load(Ordering::Relaxed) > job.wanted.load(Ordering::Relaxed) + STALE_FRAMES;
            let outcome = if stale { Outcome::Skipped } else { Outcome::Done(load(job)) };
            let _ = tx.send((key, outcome));
            ctx.request_repaint();
        });
    }
    queue
}

impl Thumbs {
    /// Uploads finished thumbnails, evicts unused textures and sets the disk cache location.
    pub fn poll(&mut self, ctx: &egui::Context, archive: &Path) {
        self.frame += 1;
        self.now.store(self.frame, Ordering::Relaxed);
        let dir = archive.join(".thumbs");
        if self.cache_dir.as_deref() != Some(&dir) {
            self.cache_dir = Some(dir);
        }
        self.pending.extend(self.rx.try_iter());
        let mut large_uploaded = false;
        let mut deferred = VecDeque::new();
        while let Some((key, outcome)) = self.pending.pop_front() {
            let img = match outcome {
                Outcome::Skipped => {
                    // Forget it so a later request queues it again.
                    self.map.remove(&key);
                    continue;
                }
                Outcome::Done(img) => img,
            };
            if img.is_some() && key.1 >= TRANSIENT_MIN {
                // A large texture upload is a visible hitch; take one per frame.
                if large_uploaded {
                    deferred.push_back((key, Outcome::Done(img)));
                    continue;
                }
                large_uploaded = true;
            }
            let (thumb, hist) = match img {
                Some((ci, hist)) => (Thumb::Ready(ctx.load_texture(key.0.to_string_lossy(), ci, Default::default())), hist),
                None => (Thumb::Missing, None),
            };
            let wanted = self.map.get(&key).map(|e| e.wanted.clone()).unwrap_or_default();
            self.map.insert(key, Entry { thumb, hist, last_used: self.frame, wanted });
        }
        if !deferred.is_empty() {
            self.pending = deferred;
            ctx.request_repaint();
        }
        self.evict();
    }

    /// Retry images that failed to load (call after files were added).
    pub fn forget_missing(&mut self) {
        self.map.retain(|_, e| !matches!(e.thumb, Thumb::Missing));
    }

    /// Drops the loaded thumbnails of `path` (all sizes) so they are decoded again, e.g. after the file changed.
    pub fn invalidate(&mut self, path: &Path) {
        self.map.retain(|k, _| k.0 != path);
    }

    fn evict(&mut self) {
        let frame = self.frame;
        // Not drawn in the previous frame.
        let idle = |e: &Entry| e.last_used + 1 < frame;
        let ready = |e: &Entry| matches!(e.thumb, Thumb::Ready(_));
        self.map.retain(|k, e| !(k.1 >= TRANSIENT_MIN && idle(e) && ready(e)));
        if self.map.len() > MAX_ENTRIES {
            let mut old: Vec<_> = self.map.iter().filter(|(_, e)| idle(e) && ready(e)).map(|(k, e)| (e.last_used, k.clone())).collect();
            old.sort();
            for (_, k) in old.into_iter().take(self.map.len() - MAX_ENTRIES) {
                self.map.remove(&k);
            }
        }
    }

    /// Histogram and black/white points of a loaded large (preview-size) image.
    pub fn stats(&self, path: &Path, size: u32) -> Option<&ImageStats> {
        self.map.get(&(path.to_path_buf(), size))?.hist.as_deref()
    }

    /// Texture of `path` scaled to fit `size` px; queues a background load on first use.
    pub fn get(&mut self, ctx: &egui::Context, path: &Path, size: u32) -> &Thumb {
        self.request(ctx, path, size, false)
    }

    /// Like `get` for an image that is likely needed soon; loaded only when the workers are otherwise idle.
    pub fn preload(&mut self, ctx: &egui::Context, path: &Path, size: u32) {
        self.request(ctx, path, size, true);
    }

    fn request(&mut self, ctx: &egui::Context, path: &Path, size: u32, low: bool) -> &Thumb {
        let key = (path.to_path_buf(), size);
        let frame = self.frame;
        if !self.map.contains_key(&key) {
            let wanted = Arc::new(AtomicU64::new(frame));
            self.map.insert(key.clone(), Entry { thumb: Thumb::Loading, hist: None, last_used: frame, wanted: wanted.clone() });
            let (tx, now) = (self.tx.clone(), self.now.clone());
            let queue = self.queue.get_or_insert_with(|| spawn_workers(tx, ctx.clone(), now));
            let job = Job { key: key.clone(), cache_dir: self.cache_dir.clone(), wanted };
            let mut jobs = queue.jobs.lock().unwrap();
            if low {
                jobs.1.push(job);
            } else {
                jobs.0.push(job);
            }
            queue.signal.notify_one();
        }
        let e = self.map.get_mut(&key).unwrap();
        e.last_used = frame;
        e.wanted.store(frame, Ordering::Relaxed);
        &e.thumb
    }
}

/// UV rectangle showing the centered part of a `tex`-sized image that fills a `view`-sized rect.
fn crop_uv(tex: egui::Vec2, view: egui::Vec2) -> egui::Rect {
    let scale = (view.x / tex.x).max(view.y / tex.y);
    let (fx, fy) = (view.x / (tex.x * scale), view.y / (tex.y * scale));
    egui::Rect::from_min_max(egui::pos2((1.0 - fx) / 2.0, (1.0 - fy) / 2.0), egui::pos2((1.0 + fx) / 2.0, (1.0 + fy) / 2.0))
}

impl Thumbs {
    /// Paint the thumbnail of `path` fitted into `rect`; spinner while loading, "missing" if unreadable.
    pub fn paint(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, path: &Path, rect: egui::Rect, size: u32) {
        self.paint_with(ui, ctx, path, rect, size, false);
    }

    /// Like `paint`, but fills `rect` completely with a center crop.
    pub fn paint_cropped(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, path: &Path, rect: egui::Rect, size: u32) {
        self.paint_with(ui, ctx, path, rect, size, true);
    }

    fn paint_with(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, path: &Path, rect: egui::Rect, size: u32, crop: bool) {
        match self.get(ctx, path, size) {
            Thumb::Ready(tex) => {
                let s = tex.size_vec2();
                if crop {
                    ui.painter().image(tex.id(), rect, crop_uv(s, rect.size()), egui::Color32::WHITE);
                } else {
                    let scale = (rect.width() / s.x).min(rect.height() / s.y);
                    let r = egui::Rect::from_center_size(rect.center(), s * scale);
                    ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
            }
            Thumb::Loading => {
                ui.put(rect, egui::Spinner::new());
            }
            Thumb::Missing => {
                ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(45));
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "missing", egui::FontId::proportional(12.0), egui::Color32::GRAY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_uv_centers() {
        // portrait image into a landscape view: full width, middle slice of the height
        let r = crop_uv(egui::vec2(100.0, 200.0), egui::vec2(130.0, 100.0));
        assert!((r.min.x - 0.0).abs() < 1e-6 && (r.max.x - 1.0).abs() < 1e-6);
        assert!((r.center().y - 0.5).abs() < 1e-6 && r.height() < 1.0);
        // landscape image: full height, middle slice of the width
        let r = crop_uv(egui::vec2(300.0, 100.0), egui::vec2(130.0, 100.0));
        assert!((r.min.y - 0.0).abs() < 1e-6 && (r.max.y - 1.0).abs() < 1e-6);
        assert!((r.center().x - 0.5).abs() < 1e-6 && r.width() < 1.0);
    }

    #[test]
    fn rotate_file_turns_pixels() {
        let dir = std::env::temp_dir().join(format!("fa-rotate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.png", "a.jpg", "a.tif"] {
            let p = dir.join(name);
            let mut im = image::RgbImage::new(2, 1);
            im.put_pixel(0, 0, image::Rgb([255, 255, 255]));
            im.save(&p).unwrap();
            rotate_file(&p, 1).unwrap();
            let r = image::open(&p).unwrap().to_rgb8();
            assert_eq!((r.width(), r.height()), (1, 2), "{name}");
            // the first pixel moved from the left to the top
            assert!(r.get_pixel(0, 0)[0] > 200 && r.get_pixel(0, 1)[0] < 50, "{name}");
            rotate_file(&p, -1).unwrap();
            assert_eq!(image::open(&p).unwrap().width(), 2, "{name}");
            rotate_file(&p, 4).unwrap();
            assert!(!p.with_extension(format!("{}.tmp", name.rsplit('.').next().unwrap())).exists());
        }
        assert!(rotate_file(&dir.join("a.gif"), 1).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_file_follows_file_changes() {
        let dir = std::env::temp_dir().join(format!("fa-thumbs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.png");
        std::fs::write(&f, b"one").unwrap();
        let a = cache_file(&dir, &f, 320).unwrap();
        assert_eq!(a, cache_file(&dir, &f, 320).unwrap());
        assert_ne!(a, cache_file(&dir, &f, 128).unwrap());
        std::fs::write(&f, b"longer content").unwrap();
        assert_ne!(a, cache_file(&dir, &f, 320).unwrap());
        assert!(cache_file(&dir, &dir.join("missing.png"), 320).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
