//! Background thumbnail loading and texture cache (native only).
//!
//! Thumbnails are decoded by a small worker pool and cached on disk below
//! `<archive>/.thumbs`, so the full-size originals are decoded only once.
//! Textures are evicted when unused.
use crate::histogram::{image_stats, ImageStats};
use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
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
        Thumbs {
            map: HashMap::new(),
            frame: 0,
            now: Arc::default(),
            cache_dir: None,
            queue: None,
            pending: VecDeque::new(),
            tx,
            rx,
        }
    }
}

/// Cache file for `path` at `size`; changes when the file's size or mtime change.
fn cache_file(dir: &Path, path: &Path, size: u32) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (path, meta.len(), mtime.as_nanos()).hash(&mut h);
    Some(
        dir.join(size.to_string())
            .join(format!("{:016x}.png", h.finish())),
    )
}

/// Format and lowercase extension of an editable image file (JPEG, PNG or TIFF).
fn editable(path: &Path) -> Result<(image::ImageFormat, String), String> {
    use image::ImageFormat;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let format = match ext.as_str() {
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "png" => ImageFormat::Png,
        "tif" | "tiff" => ImageFormat::Tiff,
        _ => return Err(format!("{}: unsupported format", path.display())),
    };
    Ok((format, ext))
}

/// Replaces the image file `path` with `im`, keeping its format.
/// The result is written to a temp file and renamed over the original; JPEGs are re-encoded (quality 95, no EXIF).
fn overwrite(
    path: &Path,
    im: &image::DynamicImage,
    format: image::ImageFormat,
    ext: &str,
) -> Result<(), String> {
    use image::{codecs::jpeg::JpegEncoder, ImageFormat};
    let tmp = path.with_extension(format!("{ext}.tmp"));
    let saved = if format == ImageFormat::Jpeg {
        std::fs::File::create(&tmp)
            .map_err(|e| e.to_string())
            .and_then(|f| {
                im.to_rgb8()
                    .write_with_encoder(JpegEncoder::new_with_quality(
                        std::io::BufWriter::new(f),
                        95,
                    ))
                    .map_err(|e| e.to_string())
            })
    } else {
        im.save_with_format(&tmp, format).map_err(|e| e.to_string())
    };
    saved
        .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| e.to_string()))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", path.display())
        })
}

/// Rotates the image file by `quarter_turns` clockwise (negative = counter-clockwise), keeping its format.
pub fn rotate_file(path: &Path, quarter_turns: i32) -> Result<(), String> {
    let (format, ext) = editable(path)?;
    let im = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let im = match quarter_turns.rem_euclid(4) {
        0 => return Ok(()),
        1 => im.rotate90(),
        2 => im.rotate180(),
        _ => im.rotate270(),
    };
    overwrite(path, &im, format, &ext)
}

/// Crops the image file to `rect` = `[left, top, right, bottom]` as fractions (0..1) of its size.
pub fn crop_file(path: &Path, rect: [f32; 4]) -> Result<(), String> {
    let (format, ext) = editable(path)?;
    let im = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (im.width() as f32, im.height() as f32);
    let edge = |f: f32, len: f32| (f.clamp(0.0, 1.0) * len).round() as u32;
    let (x0, y0) = (edge(rect[0].min(rect[2]), w), edge(rect[1].min(rect[3]), h));
    let (x1, y1) = (edge(rect[0].max(rect[2]), w), edge(rect[1].max(rect[3]), h));
    if x1 <= x0 || y1 <= y0 {
        return Err("empty crop".into());
    }
    if (x1 - x0, y1 - y0) == (im.width(), im.height()) {
        return Ok(());
    }
    overwrite(path, &im.crop_imm(x0, y0, x1 - x0, y1 - y0), format, &ext)
}

/// A spot to heal: centre as fractions of the image width and height, radius as a fraction of the width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// Spots are blended over `FEATHER` x their radius, with a full-strength core.
const FEATHER: f32 = 1.2;

/// Pixels covered by any spot (including the feathered edge).
fn spot_mask(w: u32, h: u32, spots: &[Spot]) -> Vec<bool> {
    let mut mask = vec![false; (w * h) as usize];
    for s in spots {
        let (cx, cy, reach) = (
            s.x * w as f32,
            s.y * h as f32,
            (s.r * w as f32).max(1.0) * FEATHER,
        );
        for y in
            ((cy - reach).floor() as i64).max(0)..=((cy + reach).ceil() as i64).min(h as i64 - 1)
        {
            for x in ((cx - reach).floor() as i64).max(0)
                ..=((cx + reach).ceil() as i64).min(w as i64 - 1)
            {
                if (x as f32 - cx).hypot(y as f32 - cy) <= reach {
                    mask[(y as u32 * w + x as u32) as usize] = true;
                }
            }
        }
    }
    mask
}

/// Per-channel median of a ring of pixels around the spot: the flat fallback when no texture can be copied.
fn median_fill(im: &image::RgbImage, cx: f32, cy: f32, r: f32) -> Option<[f32; 3]> {
    const ANGLES: usize = 24;
    let (w, h) = (im.width() as i64, im.height() as i64);
    let mut samples: [Vec<u8>; 3] = Default::default();
    for ring in [1.4_f32, 1.7, 2.0] {
        for a in 0..ANGLES {
            let t = (a as f32 + ring) / ANGLES as f32 * std::f32::consts::TAU;
            let (x, y) = (
                (cx + t.cos() * r * ring).round() as i64,
                (cy + t.sin() * r * ring).round() as i64,
            );
            if (0..w).contains(&x) && (0..h).contains(&y) {
                let p = im.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    samples[c].push(p[c]);
                }
            }
        }
    }
    if samples[0].len() < 8 {
        return None;
    }
    let mut fill = [0.0; 3];
    for (f, v) in fill.iter_mut().zip(samples.iter_mut()) {
        v.sort_unstable();
        *f = v[v.len() / 2] as f32;
    }
    Some(fill)
}

/// Offsets (on a grid of `stride`) of the pixels whose distance from the centre is within `lo..=hi`.
fn ring_offsets(lo: f32, hi: f32, stride: i64) -> Vec<(i64, i64)> {
    let n = (hi / stride as f32).ceil() as i64;
    let mut out = vec![];
    for j in -n..=n {
        for i in -n..=n {
            let (x, y) = (i * stride, j * stride);
            let d = (x as f32).hypot(y as f32);
            if d >= lo && d <= hi {
                out.push((x, y));
            }
        }
    }
    out
}

/// Offset `(dx, dy)` of the nearby area that looks most like the surroundings of the spot at `(cx, cy)`,
/// and the per-channel mean difference (spot surroundings minus source) for matching brightness.
/// `mask` marks pixels that must not be used (spots still to be healed).
fn best_source(
    im: &image::RgbImage,
    mask: &[bool],
    cx: f32,
    cy: f32,
    r: f32,
) -> Option<((i64, i64), [f32; 3])> {
    let (w, h) = (im.width() as i64, im.height() as i64);
    let inner = r * FEATHER;
    let outer = inner + (r * 0.6).max(3.0);
    let stride = ((r / 6.0) as i64).max(1);
    let usable =
        |x: i64, y: i64| (0..w).contains(&x) && (0..h).contains(&y) && !mask[(y * w + x) as usize];
    let (px, py) = (cx.round() as i64, cy.round() as i64);
    // Context: pixels around the spot that are themselves intact. Disc: what a candidate must supply.
    let context: Vec<(i64, i64)> = ring_offsets(inner, outer, stride)
        .into_iter()
        .filter(|&(x, y)| usable(px + x, py + y))
        .collect();
    if context.len() < 12 {
        return None;
    }
    let disc = ring_offsets(0.0, inner, stride);
    let min_d = 2.0 * outer;
    let max_d = (8.0 * r).max(min_d + 8.0);
    let step = ((r / 2.0) as i64).max(1);
    let n = (max_d / step as f32).ceil() as i64;
    let mut best: Option<(f64, (i64, i64))> = None;
    for j in -n..=n {
        for i in -n..=n {
            let (dx, dy) = (i * step, j * step);
            let dist = (dx as f32).hypot(dy as f32);
            if dist < min_d || dist > max_d {
                continue;
            }
            if !disc
                .iter()
                .chain(&context)
                .all(|&(x, y)| usable(px + dx + x, py + dy + y))
            {
                continue;
            }
            let mut ssd = 0.0_f64;
            for &(x, y) in &context {
                let (a, b) = (
                    im.get_pixel((px + x) as u32, (py + y) as u32),
                    im.get_pixel((px + dx + x) as u32, (py + dy + y) as u32),
                );
                for c in 0..3 {
                    let d = a[c] as f64 - b[c] as f64;
                    ssd += d * d;
                }
                if best.is_some_and(|(s, _)| ssd >= s) {
                    break;
                }
            }
            if best.is_none_or(|(s, _)| ssd < s) {
                best = Some((ssd, (dx, dy)));
            }
        }
    }
    let (_, (dx, dy)) = best?;
    let mut delta = [0.0_f32; 3];
    for &(x, y) in &context {
        let (a, b) = (
            im.get_pixel((px + x) as u32, (py + y) as u32),
            im.get_pixel((px + dx + x) as u32, (py + dy + y) as u32),
        );
        for c in 0..3 {
            delta[c] += (a[c] as f32 - b[c] as f32) / context.len() as f32;
        }
    }
    Some(((dx, dy), delta))
}

/// Heals each spot by copying the nearby texture that best continues its surroundings (grain included),
/// feathered at the edge; falls back to a flat median fill where no such area exists.
/// Spots are healed in order and later ones may use the healed pixels as context.
fn heal(im: &mut image::RgbImage, spots: &[Spot]) {
    let (w, h) = (im.width() as i64, im.height() as i64);
    let mut mask = spot_mask(im.width(), im.height(), spots);
    for s in spots {
        let (cx, cy, r) = (s.x * w as f32, s.y * h as f32, (s.r * w as f32).max(1.0));
        let source = best_source(im, &mask, cx, cy, r);
        let flat = if source.is_none() {
            median_fill(im, cx, cy, r)
        } else {
            None
        };
        if source.is_some() || flat.is_some() {
            let reach = r * FEATHER;
            for y in ((cy - reach).floor() as i64).max(0)..=((cy + reach).ceil() as i64).min(h - 1)
            {
                for x in
                    ((cx - reach).floor() as i64).max(0)..=((cx + reach).ceil() as i64).min(w - 1)
                {
                    let a = ((reach - (x as f32 - cx).hypot(y as f32 - cy)) / (reach - r))
                        .clamp(0.0, 1.0);
                    if a <= 0.0 {
                        continue;
                    }
                    let fill = match (source, flat) {
                        (Some(((dx, dy), delta)), _) => {
                            let (sx, sy) = (x + dx, y + dy);
                            if !(0..w).contains(&sx) || !(0..h).contains(&sy) {
                                continue;
                            }
                            let p = im.get_pixel(sx as u32, sy as u32);
                            [
                                p[0] as f32 + delta[0],
                                p[1] as f32 + delta[1],
                                p[2] as f32 + delta[2],
                            ]
                        }
                        (None, Some(f)) => f,
                        _ => continue,
                    };
                    let p = im.get_pixel_mut(x as u32, y as u32);
                    for c in 0..3 {
                        p[c] = (p[c] as f32 * (1.0 - a) + fill[c] * a)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        // The healed area is intact now and may serve as context for later spots.
        let reach = r * FEATHER;
        for y in ((cy - reach).floor() as i64).max(0)..=((cy + reach).ceil() as i64).min(h - 1) {
            for x in ((cx - reach).floor() as i64).max(0)..=((cx + reach).ceil() as i64).min(w - 1)
            {
                if (x as f32 - cx).hypot(y as f32 - cy) <= reach {
                    mask[(y * w + x) as usize] = false;
                }
            }
        }
    }
}

/// Removes dust and scratch spots from the image file (8 bits per channel afterwards).
pub fn heal_file(path: &Path, spots: &[Spot]) -> Result<(), String> {
    let (format, ext) = editable(path)?;
    if spots.is_empty() {
        return Ok(());
    }
    let mut rgb = image::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .to_rgb8();
    heal(&mut rgb, spots);
    overwrite(path, &image::DynamicImage::ImageRgb8(rgb), format, &ext)
}

/// Where the untouched original of `path` is kept: `<archive>/.originals/<path relative to the archive>`.
fn original_path(archive: &Path, path: &Path) -> Result<PathBuf, String> {
    let rel = path
        .strip_prefix(archive)
        .map_err(|_| format!("{}: outside the archive", path.display()))?;
    Ok(archive.join(".originals").join(rel))
}

/// Whether a backup of the original of `path` exists.
pub fn has_original(archive: &Path, path: &Path) -> bool {
    original_path(archive, path).is_ok_and(|o| o.exists())
}

/// Copies `path` to `.originals` before the first destructive edit; an existing backup is kept.
pub fn backup_original(archive: &Path, path: &Path) -> Result<(), String> {
    let o = original_path(archive, path)?;
    if o.exists() {
        return Ok(());
    }
    let err = |e: std::io::Error| format!("backup {}: {e}", path.display());
    std::fs::create_dir_all(o.parent().unwrap_or(archive)).map_err(err)?;
    std::fs::copy(path, &o).map(|_| ()).map_err(err)
}

/// Puts the backed-up original back over `path` and removes the backup.
pub fn restore_original(archive: &Path, path: &Path) -> Result<(), String> {
    let o = original_path(archive, path)?;
    let err = |e: std::io::Error| format!("restore {}: {e}", path.display());
    let tmp = path.with_extension("restore.tmp");
    std::fs::copy(&o, &tmp).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)?;
    let _ = std::fs::remove_file(&o);
    Ok(())
}

fn to_color_image(im: &image::RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [im.width() as usize, im.height() as usize],
        im.as_raw(),
    )
}

fn load(job: Job) -> Payload {
    let (path, size) = &job.key;
    let cached = if *size <= DISK_CACHE_MAX {
        job.cache_dir
            .as_deref()
            .and_then(|d| cache_file(d, path, *size))
    } else {
        None
    };
    if let Some(c) = &cached {
        if let Ok(im) = image::open(c) {
            return Some((to_color_image(&im.to_rgba8()), None));
        }
    }
    let im = image::open(path).ok()?;
    let im = if im.width() > *size || im.height() > *size {
        im.resize(*size, *size, image::imageops::FilterType::Triangle)
    } else {
        im
    }
    .to_rgba8();
    if let Some(c) = cached {
        if let Some(dir) = c.parent() {
            let tmp = c.with_extension("tmp");
            // Best effort: a failed cache write only costs a re-decode later.
            if std::fs::create_dir_all(dir).is_ok()
                && im.save_with_format(&tmp, image::ImageFormat::Png).is_ok()
            {
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
            let stale =
                now.load(Ordering::Relaxed) > job.wanted.load(Ordering::Relaxed) + STALE_FRAMES;
            let outcome = if stale {
                Outcome::Skipped
            } else {
                Outcome::Done(load(job))
            };
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
                Some((ci, hist)) => (
                    Thumb::Ready(ctx.load_texture(key.0.to_string_lossy(), ci, Default::default())),
                    hist,
                ),
                None => (Thumb::Missing, None),
            };
            let wanted = self
                .map
                .get(&key)
                .map(|e| e.wanted.clone())
                .unwrap_or_default();
            self.map.insert(
                key,
                Entry {
                    thumb,
                    hist,
                    last_used: self.frame,
                    wanted,
                },
            );
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
        self.map
            .retain(|k, e| !(k.1 >= TRANSIENT_MIN && idle(e) && ready(e)));
        if self.map.len() > MAX_ENTRIES {
            let mut old: Vec<_> = self
                .map
                .iter()
                .filter(|(_, e)| idle(e) && ready(e))
                .map(|(k, e)| (e.last_used, k.clone()))
                .collect();
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
            self.map.insert(
                key.clone(),
                Entry {
                    thumb: Thumb::Loading,
                    hist: None,
                    last_used: frame,
                    wanted: wanted.clone(),
                },
            );
            let (tx, now) = (self.tx.clone(), self.now.clone());
            let queue = self
                .queue
                .get_or_insert_with(|| spawn_workers(tx, ctx.clone(), now));
            let job = Job {
                key: key.clone(),
                cache_dir: self.cache_dir.clone(),
                wanted,
            };
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
    egui::Rect::from_min_max(
        egui::pos2((1.0 - fx) / 2.0, (1.0 - fy) / 2.0),
        egui::pos2((1.0 + fx) / 2.0, (1.0 + fy) / 2.0),
    )
}

impl Thumbs {
    /// Paint the thumbnail of `path` fitted into `rect`; spinner while loading, "missing" if unreadable.
    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        path: &Path,
        rect: egui::Rect,
        size: u32,
    ) {
        self.paint_with(ui, ctx, path, rect, size, false);
    }

    /// Like `paint`, but fills `rect` completely with a center crop.
    pub fn paint_cropped(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        path: &Path,
        rect: egui::Rect,
        size: u32,
    ) {
        self.paint_with(ui, ctx, path, rect, size, true);
    }

    fn paint_with(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        path: &Path,
        rect: egui::Rect,
        size: u32,
        crop: bool,
    ) {
        match self.get(ctx, path, size) {
            Thumb::Ready(tex) => {
                let s = tex.size_vec2();
                if crop {
                    ui.painter().image(
                        tex.id(),
                        rect,
                        crop_uv(s, rect.size()),
                        egui::Color32::WHITE,
                    );
                } else {
                    let scale = (rect.width() / s.x).min(rect.height() / s.y);
                    let r = egui::Rect::from_center_size(rect.center(), s * scale);
                    ui.painter().image(
                        tex.id(),
                        r,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
            Thumb::Loading => {
                ui.put(rect, egui::Spinner::new());
            }
            Thumb::Missing => {
                ui.painter()
                    .rect_filled(rect, 4.0, egui::Color32::from_gray(45));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "missing",
                    egui::FontId::proportional(12.0),
                    egui::Color32::GRAY,
                );
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
        let dir = std::env::temp_dir().join(format!("latent-rotate-{}", std::process::id()));
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
            assert!(
                r.get_pixel(0, 0)[0] > 200 && r.get_pixel(0, 1)[0] < 50,
                "{name}"
            );
            rotate_file(&p, -1).unwrap();
            assert_eq!(image::open(&p).unwrap().width(), 2, "{name}");
            rotate_file(&p, 4).unwrap();
            assert!(!p
                .with_extension(format!("{}.tmp", name.rsplit('.').next().unwrap()))
                .exists());
        }
        assert!(rotate_file(&dir.join("a.gif"), 1).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn crop_and_restore() {
        let dir = std::env::temp_dir().join(format!("latent-crop-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("images")).unwrap();
        for name in ["a.png", "a.jpg"] {
            let p = dir.join("images").join(name);
            let mut im = image::RgbImage::new(10, 10);
            im.put_pixel(5, 5, image::Rgb([255, 255, 255]));
            im.save(&p).unwrap();
            assert!(!has_original(&dir, &p));
            backup_original(&dir, &p).unwrap();
            crop_file(&p, [0.5, 0.5, 1.0, 1.0]).unwrap();
            let r = image::open(&p).unwrap().to_rgb8();
            assert_eq!((r.width(), r.height()), (5, 5), "{name}");
            assert!(r.get_pixel(0, 0)[0] > 200, "{name}");
            // a second backup keeps the first original
            backup_original(&dir, &p).unwrap();
            assert!(has_original(&dir, &p));
            restore_original(&dir, &p).unwrap();
            assert_eq!(image::open(&p).unwrap().width(), 10, "{name}");
            assert!(!has_original(&dir, &p));
            assert!(crop_file(&p, [0.5, 0.5, 0.5, 0.9]).is_err());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn heal_copies_texture_instead_of_flat_fill() {
        // Periodic grain: a flat fill would differ, the right nearby area reproduces it exactly.
        let pattern = |x: u32, y: u32| {
            let v = 40 + ((x * 3 + y * 5) % 7) as u8 * 25;
            image::Rgb([v, v / 2 + 20, 255 - v])
        };
        let clean = image::RgbImage::from_fn(120, 120, pattern);
        let mut im = clean.clone();
        for y in 58..63 {
            for x in 58..63 {
                im.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let spot = Spot {
            x: 60.0 / 120.0,
            y: 60.0 / 120.0,
            r: 0.05,
        };
        heal(&mut im, &[spot]);
        assert_eq!(im, clean);
    }

    #[test]
    fn heal_does_not_copy_from_other_spots() {
        let mut im = image::RgbImage::from_pixel(120, 120, image::Rgb([90, 90, 90]));
        // two white blobs side by side: the second must not be used as the source for the first
        for y in 55..66 {
            for x in 30..42 {
                im.put_pixel(x, y, image::Rgb([255, 255, 255]));
                im.put_pixel(x + 24, y, image::Rgb([255, 255, 255]));
            }
        }
        let spots = [
            Spot {
                x: 36.0 / 120.0,
                y: 60.0 / 120.0,
                r: 0.07,
            },
            Spot {
                x: 60.0 / 120.0,
                y: 60.0 / 120.0,
                r: 0.07,
            },
        ];
        heal(&mut im, &spots);
        assert!(im.pixels().all(|p| p.0 == [90, 90, 90]));
    }

    #[test]
    fn heal_removes_speck_and_keeps_rest() {
        let mut im = image::RgbImage::from_pixel(100, 100, image::Rgb([100, 120, 140]));
        for (x, y) in [(50, 50), (51, 50), (50, 51), (51, 51)] {
            im.put_pixel(x, y, image::Rgb([255, 255, 255]));
        }
        im.put_pixel(5, 5, image::Rgb([0, 0, 0]));
        heal(
            &mut im,
            &[Spot {
                x: 0.505,
                y: 0.505,
                r: 0.03,
            }],
        );
        assert_eq!(im.get_pixel(50, 50).0, [100, 120, 140]);
        assert_eq!(im.get_pixel(51, 51).0, [100, 120, 140]);
        assert_eq!(im.get_pixel(5, 5).0, [0, 0, 0]);
        // a spot at the corner has fewer samples but must not panic
        heal(
            &mut im,
            &[
                Spot {
                    x: 0.0,
                    y: 0.0,
                    r: 0.05,
                },
                Spot {
                    x: 1.0,
                    y: 1.0,
                    r: 0.5,
                },
            ],
        );
    }

    #[test]
    fn cache_file_follows_file_changes() {
        let dir = std::env::temp_dir().join(format!("latent-thumbs-test-{}", std::process::id()));
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
