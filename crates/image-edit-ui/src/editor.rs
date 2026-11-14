//! The image view of a saved scan: adjust, sharpen, rotate and crop with a live histogram,
//! modelled on the image view of latent.
//!
//! Edits only change the preview. **Apply** writes them to the file (at full resolution, same
//! format, 16 bit kept); the untouched original can be saved first with **Backup original**
//! (`.originals/…`, latent's layout, see [`crate::edit`]).

use crate::crop::Grab;
pub use crate::crop::{Crop, FULL};
use crate::dust;
use crate::edit;
use crate::histogram::{marker_points, Histogram, Markers};
use crate::preview::{self, rotate_image, to_color_image};
use crate::view::ZoomState;
use egui::{ColorImage, Context, TextureHandle};
use latent_image_edit::clarity::{clarity, Clarity, ClarityStage};
use latent_image_edit::heal::{heal, Spot};
use latent_image_edit::sharpen::{sharpen, Sharpen, SharpenStage};
use latent_image_edit::tone::{Adjust, Tone, ToneOptions, ToneStage};
use latent_image_edit::RawImage;
use latent_image_edit::{file_depth, read_image, write_image_depth, BitDepth, Pipeline, Rotate};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Longest side of the preview decoded for the editor.
pub const PREVIEW_SIDE: u32 = 2048;

/// What the user changed and has not applied yet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Edits {
    /// Invert and balance a negative.
    pub negative: bool,
    pub adjust: Adjust,
    pub clarity: Clarity,
    pub sharpen: Sharpen,
    /// Clockwise quarter turns.
    pub turns: u8,
    /// Crop, normalized in the file's own orientation (before the turns).
    pub crop: Option<Crop>,
    /// Dust spots to heal, in the file's own orientation (see [`crate::dust`]).
    pub spots: Vec<Spot>,
    /// Write the file with 8 instead of 16 bits per sample (an 8-bit file stays 8 bit).
    pub eight_bit: bool,
}

impl Edits {
    /// Nothing that would change the file (a full-size crop or a radius change without
    /// sharpening counts as nothing).
    pub fn is_neutral(&self) -> bool {
        !self.negative
            && self.adjust.is_neutral()
            && self.clarity.is_neutral()
            && self.sharpen.is_neutral()
            && self.turns.is_multiple_of(4)
            && self.crop.is_none_or(|c| c == FULL)
            && self.spots.is_empty()
            && !self.eight_bit
    }

    pub fn degrees(&self) -> u16 {
        (self.turns % 4) as u16 * 90
    }

    pub fn tone(&self) -> ToneOptions {
        ToneOptions {
            negative: self.negative,
            auto_levels: false,
            adjust: self.adjust,
        }
    }

    /// The part of the edits that decides how the preview looks. The crop only matters while
    /// the negative base is estimated from it, so dragging the crop of a positive does not
    /// re-render the picture.
    fn render_key(&self) -> Self {
        Self {
            crop: self.crop.filter(|_| self.negative),
            sharpen: if self.sharpen.is_neutral() {
                Sharpen::default()
            } else {
                self.sharpen
            },
            eight_bit: false,
            ..self.clone()
        }
    }
}

/// TIFF, PNG and JPEG can be edited and written back. JPEG stays 8 bit and is re-compressed by
/// every Apply.
pub fn is_editable(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "tif" | "tiff" | "png" | "jpg" | "jpeg"))
}

/// The tone the edits give `base`, estimated on `base` (the negative base comes from the crop,
/// as it will when applied); `None` when the edits leave the tone alone. Part of the picture
/// (see [`Detail`]) is toned with the estimate of the whole preview, so it matches it.
pub fn tone_for(base: &RawImage, edits: &Edits) -> Option<Tone> {
    let tone = edits.tone();
    if tone.is_identity() {
        return None;
    }
    let reference = edits.crop.and_then(|c| preview::region(base, c));
    Some(Tone::estimate(reference.as_ref().unwrap_or(base), tone))
}

/// The edited preview (without the crop, which is an overlay until it is applied). `scale` is the
/// preview's size relative to the file, so the sharpening radius matches the final result.
pub fn render(base: &RawImage, edits: &Edits, scale: f32) -> ColorImage {
    let mut img = base.clone();
    if let Some(tone) = tone_for(base, edits) {
        tone.apply(&mut img);
    }
    clarity(&mut img, edits.clarity);
    sharpen(&mut img, edits.sharpen.scaled(scale));
    rotate_image(&to_color_image(&img, u32::MAX), edits.degrees())
}

fn to_rgba(img: &ColorImage) -> image::RgbaImage {
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
    image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, bytes)
        .expect("a colour image has width x height pixels")
}

/// Warmth and tint that make the picture neutral at `(u, v)` (fractions of the preview): the mean
/// of a small patch of `base` goes through the film step, which is what the colour gains see.
/// `None` for gray pictures.
pub fn neutral_gray(base: &RawImage, edits: &Edits, u: f32, v: f32) -> Option<(f32, f32)> {
    if base.channels != 3 || base.width == 0 || base.height == 0 {
        return None;
    }
    let (w, h) = (base.width as f32, base.height as f32);
    let reach = (w.max(h) * 0.002).round().max(2.0);
    let (cx, cy) = (u.clamp(0.0, 1.0) * w, v.clamp(0.0, 1.0) * h);
    let frame = [
        (cx - reach) / w,
        (cy - reach) / h,
        (cx + reach) / w,
        (cy + reach) / h,
    ];
    let patch = preview::region(base, frame.map(|f| f.clamp(0.0, 1.0)))?;
    let n = (patch.width * patch.height) as f32;
    let mean = |c: usize| {
        patch
            .data
            .iter()
            .skip(c)
            .step_by(3)
            .map(|&v| v as f32)
            .sum::<f32>()
            / n
    };
    let mut px = RawImage::new(
        1,
        1,
        3,
        (0..3).map(|c| mean(c).round() as u16).collect::<Vec<_>>(),
    )
    .ok()?;
    let film = ToneOptions {
        negative: edits.negative,
        auto_levels: false,
        adjust: Adjust::default(),
    };
    if !film.is_identity() {
        // The negative base comes from the crop or the whole picture, as in `render`.
        let reference = edits.crop.and_then(|c| preview::region(base, c));
        Tone::estimate(reference.as_ref().unwrap_or(base), film).apply(&mut px);
    }
    Some(Adjust::balance_for([
        px.data[0] as f32,
        px.data[1] as f32,
        px.data[2] as f32,
    ]))
}

/// Black and white point measured on the crop (or the whole preview).
pub fn auto_levels(base: &RawImage, edits: &Edits) -> Adjust {
    let reference = edits.crop.and_then(|c| preview::region(base, c));
    Adjust::auto_levels(reference.as_ref().unwrap_or(base), edits.negative)
}

/// Applies `edits` to the file at full resolution: heal dust, crop, rotate, tone, clarity, sharpen, written over
/// the file atomically in the same format. Returns the image that was written.
pub fn apply_file(path: &Path, edits: &Edits) -> Result<RawImage, String> {
    let depth = if edits.eight_bit || file_depth(path) == Some(BitDepth::Eight) {
        BitDepth::Eight
    } else {
        BitDepth::Sixteen
    };
    let mut img = read_image(path).map_err(|e| e.to_string())?;
    // Healing comes first: the spots are in the file's own orientation and pixels.
    heal(&mut img, &edits.spots);
    if let Some(c) = edits.crop.filter(|c| *c != FULL) {
        img = preview::region(&img, c).ok_or("the crop is empty")?;
    }
    let pipeline = Pipeline::default()
        .with(Rotate(edits.degrees()))
        .with(ToneStage(edits.tone()))
        .with(ClarityStage(edits.clarity))
        .with(SharpenStage(edits.sharpen));
    let out = pipeline.run(img).map_err(|e| e.to_string())?;
    edit::overwrite_atomic(path, |tmp| {
        write_image_depth(&out, tmp, depth).map_err(|e| e.to_string())
    })?;
    Ok(out)
}

/// What a background file operation of the editor does.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorOp {
    Apply { edits: Edits, backup_first: bool },
    Backup,
    Restore,
}

impl EditorOp {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Apply { .. } => "Apply",
            Self::Backup => "Backup",
            Self::Restore => "Restore",
        }
    }
}

/// The work of an [`EditorOp`] on the calling thread (hosts run it on a worker). An apply
/// returns the image written.
pub fn run_op(root: &Path, path: &Path, op: EditorOp) -> Result<Option<RawImage>, String> {
    match op {
        EditorOp::Backup => edit::backup_original(root, path).map(|()| None),
        EditorOp::Restore => edit::restore_original(root, path).map(|()| None),
        EditorOp::Apply {
            edits,
            backup_first,
        } => {
            if backup_first {
                edit::backup_original(root, path)?;
            }
            apply_file(path, &edits).map(Some)
        }
    }
}

/// One image open in the editor.
pub struct EditorState {
    pub rel: String,
    pub path: PathBuf,
    /// The decoded preview (16 bit, at most [`PREVIEW_SIDE`] long); `None` while loading.
    pub base: Option<Arc<RawImage>>,
    pub file_size: [u32; 2],
    /// Preview size relative to the file.
    pub scale: f32,
    pub tex: Option<TextureHandle>,
    shown: Option<Edits>,
    pub histogram: Histogram,
    /// Darkest, brightest and most neutral spot of the rendered preview.
    pub markers: Option<Markers>,
    pub edits: Edits,
    /// Zoom and tool state for hosts that show the editor in a window of their own (latent-scan).
    /// latent's image view keeps the zoom in its own state, shared with View mode.
    pub zoom: Option<ZoomState>,
    /// The pane's shape of the last frame (for the zoom buttons).
    pub shape: f32,
    pub grab: Option<Grab>,
    /// Dust mode: dragging over the picture marks spots; the brush radius is a fraction of the
    /// picture's width.
    pub dust: bool,
    pub brush: f32,
    /// The pointer is down while marking dust or dragging the crop: the preview waits until the
    /// edit is complete.
    pub painting: bool,
    /// Asking whether to overwrite without a backup.
    pub confirm: bool,
    /// The picture at full size and halved sizes, for the sharp part (see [`Loaded`]).
    levels: Vec<Arc<RawImage>>,
    /// The picture is too big to keep at full size: only the preview is available.
    pub limited: bool,
    /// The sharp part of the picture when zoomed in beyond the preview.
    pub detail: Option<Detail>,
    /// The sharp part asked for: edits, region, picture generation, level scale.
    detail_asked: Option<(Edits, Crop, u64, f32)>,
    detail_renderer: Option<DetailRenderer>,
    /// What the view looked like when it last changed, and when (seconds): the sharp part waits
    /// until it rests.
    detail_quiet: Option<(Edits, [i32; 4], f64)>,
    /// Screen points per file pixel of the last frame (1.0 = every file pixel on screen).
    pub pixel_zoom: f32,
    renderer: Option<Renderer>,
    /// The edits the render thread is working on.
    requested: Option<Edits>,
    /// Counts loaded pictures, so a late preview of an older one is ignored.
    generation: u64,
    pub editable: bool,
    pub has_original: bool,
    /// Bits per sample of the file on disk.
    pub source_depth: BitDepth,
    pub busy: Option<&'static str>,
    pub error: Option<String>,
}

impl EditorState {
    pub fn new(root: &Path, rel: &str) -> Self {
        let path = root.join(rel);
        Self {
            rel: rel.to_string(),
            editable: is_editable(&path),
            has_original: edit::has_original(root, &path),
            source_depth: file_depth(&path).unwrap_or_default(),
            path,
            base: None,
            file_size: [0, 0],
            scale: 1.0,
            tex: None,
            shown: None,
            histogram: Histogram::default(),
            markers: None,
            edits: Edits::default(),
            zoom: None,
            shape: 1.0,
            grab: None,
            dust: false,
            brush: dust::DEFAULT_BRUSH,
            painting: false,
            confirm: false,
            levels: vec![],
            limited: false,
            detail: None,
            detail_asked: None,
            detail_renderer: None,
            detail_quiet: None,
            pixel_zoom: 0.0,
            renderer: None,
            requested: None,
            generation: 0,
            busy: None,
            error: None,
        }
    }

    pub fn dirty(&self) -> bool {
        !self.edits.is_neutral()
    }

    /// The decoded preview arrived.
    pub fn loaded(&mut self, preview: RawImage, file_size: [u32; 2]) {
        self.scale = preview.width as f32 / file_size[0].max(1) as f32;
        self.file_size = file_size;
        self.base = Some(Arc::new(preview));
        self.drop_detail();
        self.shown = None;
        self.requested = None;
        self.generation += 1;
        self.busy = None;
        self.error = None;
    }

    /// Forgets the levels and the sharp part of the picture that was shown.
    fn drop_detail(&mut self) {
        self.levels.clear();
        self.limited = false;
        self.detail = None;
        self.detail_asked = None;
        self.detail_quiet = None;
    }

    /// Back to the file as it is (after an apply or a restore): the preview is decoded again.
    pub fn reset_for_reload(&mut self, root: &Path) {
        self.edits = Edits::default();
        self.dust = false;
        self.confirm = false;
        self.base = None;
        self.drop_detail();
        self.shown = None;
        self.requested = None;
        self.generation += 1;
        self.has_original = edit::has_original(root, &self.path);
        self.source_depth = file_depth(&self.path).unwrap_or_default();
    }

    /// The preview as displayed: `(width, height)` of the picture after the turns.
    pub fn picture_size(&self) -> Option<[f32; 2]> {
        let b = self.base.as_ref()?;
        let (w, h) = (b.width as f32, b.height as f32);
        Some(if self.edits.turns.is_multiple_of(2) {
            [w, h]
        } else {
            [h, w]
        })
    }

    /// Asks the render thread for a new preview when the edits changed what is shown, and
    /// shows the ones that are ready.
    pub fn refresh(&mut self, ctx: &Context) {
        self.take_rendered(ctx);
        if self.grab.is_some() || self.painting {
            return;
        }
        let Some(base) = &self.base else { return };
        let key = self.edits.render_key();
        if self.shown.as_ref() == Some(&key) || self.requested.as_ref() == Some(&key) {
            return;
        }
        let job = RenderJob {
            generation: self.generation,
            key: key.clone(),
            base: base.clone(),
            scale: self.scale,
        };
        let renderer = self.renderer.get_or_insert_with(|| Renderer::new(ctx));
        if renderer.tx.send(job).is_ok() {
            self.requested = Some(key);
        }
    }

    /// A preview is on its way.
    pub fn rendering(&self) -> bool {
        self.requested.is_some()
    }

    /// Uploads the previews the render thread finished (the newest one wins).
    fn take_rendered(&mut self, ctx: &Context) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        while let Ok(out) = renderer.rx.try_recv() {
            if out.generation != self.generation || self.requested.as_ref() != Some(&out.key) {
                continue;
            }
            self.histogram = out.histogram;
            self.markers = Some(out.markers);
            match &mut self.tex {
                Some(tex) => tex.set(out.image, Default::default()),
                None => {
                    self.tex = Some(ctx.load_texture(
                        format!("editor:{}", self.rel),
                        out.image,
                        Default::default(),
                    ))
                }
            }
            self.shown = Some(out.key);
            self.requested = None;
        }
    }
}

/// What the render thread is asked to make: the preview for one set of edits.
struct RenderJob {
    /// Which loaded picture the job belongs to (a reload starts a new one).
    generation: u64,
    key: Edits,
    base: Arc<RawImage>,
    scale: f32,
}

/// The finished preview.
struct Rendered {
    generation: u64,
    key: Edits,
    image: ColorImage,
    histogram: Histogram,
    markers: Markers,
}

/// Renders the preview on its own thread so a slider drag never blocks the window. Only the
/// newest request is rendered; older ones that are still waiting are dropped.
struct Renderer {
    tx: Sender<RenderJob>,
    rx: Receiver<Rendered>,
}

impl Renderer {
    fn new(ctx: &Context) -> Self {
        let (tx, jobs) = channel::<RenderJob>();
        let (done, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut healed = HealCache::default();
            while let Ok(mut job) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let out =
                    crate::trace::timed("editor preview render", || render_job(job, &mut healed));
                if done.send(out).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self { tx, rx }
    }
}

/// The preview base with the spots healed, for the picture and spots it was made for.
#[derive(Default)]
struct HealCache(Option<(Arc<RawImage>, Vec<Spot>, RawImage)>);

impl HealCache {
    fn base<'a>(&'a mut self, base: &'a Arc<RawImage>, spots: &[Spot]) -> &'a RawImage {
        if spots.is_empty() {
            return base;
        }
        let fresh = self
            .0
            .as_ref()
            .is_some_and(|(b, s, _)| Arc::ptr_eq(b, base) && s == spots);
        if !fresh {
            // Healing searches for texture, so it only runs when the spots changed.
            let mut img = (**base).clone();
            heal(&mut img, spots);
            self.0 = Some((base.clone(), spots.to_vec(), img));
        }
        &self.0.as_ref().expect("just made").2
    }
}

/// The work of the render thread for one job.
fn render_job(job: RenderJob, healed: &mut HealCache) -> Rendered {
    let base = healed.base(&job.base, &job.key.spots);
    let image = render(base, &job.key, job.scale);
    Rendered {
        generation: job.generation,
        histogram: Histogram::of(&image),
        markers: marker_points(&to_rgba(&image)),
        key: job.key,
        image,
    }
}

/// Most memory (bytes) the decoded levels of one picture may take. A bigger picture is edited on
/// the preview alone.
pub const FULL_MAX_BYTES: usize = 1 << 30;

/// A decoded file: the preview, the file's size and, for detail views, the picture at full size
/// and halved sizes (the "pyramid"), biggest first, down to the first below the preview.
pub struct Loaded {
    /// At most [`PREVIEW_SIDE`] long.
    pub preview: RawImage,
    pub size: [u32; 2],
    /// Levels longer than the preview; empty when the picture is not bigger than the preview or
    /// would take more than the memory cap ([`Loaded::limited`]).
    pub levels: Vec<Arc<RawImage>>,
    /// The picture is bigger than the memory cap allows: no detail view.
    pub limited: bool,
}

/// Reads the file and makes its [`Loaded`], keeping the levels while they fit `cap` bytes.
pub fn decode_with_cap(path: &Path, cap: usize) -> Result<Loaded, String> {
    let full = read_image(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let size = [full.width, full.height];
    let preview = preview::downscale(&full, PREVIEW_SIDE);
    if full.width.max(full.height) <= PREVIEW_SIDE {
        return Ok(Loaded {
            preview,
            size,
            levels: vec![],
            limited: false,
        });
    }
    // The halved levels add a third to the full picture.
    let bytes = full.data.len() * std::mem::size_of::<u16>() / 3 * 4;
    if bytes > cap {
        return Ok(Loaded {
            preview,
            size,
            levels: vec![],
            limited: true,
        });
    }
    let mut levels = vec![Arc::new(full)];
    loop {
        let last = &levels[levels.len() - 1];
        let side = last.width.max(last.height) / 2;
        if side <= PREVIEW_SIDE {
            break;
        }
        let next = preview::downscale(last, side);
        levels.push(Arc::new(next));
    }
    Ok(Loaded {
        preview,
        size,
        levels,
        limited: false,
    })
}

/// [`decode_with_cap`] with the standard cap.
pub fn decode(path: &Path) -> Result<Loaded, String> {
    decode_with_cap(path, FULL_MAX_BYTES)
}

/// Decodes the file on a thread (see [`decode`]).
pub fn load_async(ctx: &Context, path: PathBuf) -> Receiver<Result<Loaded, String>> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(decode(&path));
        ctx.request_repaint();
    });
    rx
}

/// How long the view and the edits must rest before the sharp part is rendered (seconds).
const DETAIL_DELAY: f64 = 0.15;
/// Share of the visible part added on every side, so small pans stay inside the sharp part.
const DETAIL_MARGIN: f32 = 0.25;
/// Longest side (pixels) of a sharp part; a texture must stay below the GPU limit.
const DETAIL_MAX_SIDE: u32 = 8192;

/// A part of the picture rendered from a pyramid level, drawn over the preview when zoomed in.
pub struct Detail {
    pub tex: TextureHandle,
    /// The part of the picture the texture covers, normalized `[l, t, r, b]` in the file.
    pub window: Crop,
    /// The edits it shows.
    key: Edits,
    generation: u64,
    /// Level pixels per file pixel.
    scale: f32,
}

struct DetailJob {
    generation: u64,
    key: Edits,
    level: Arc<RawImage>,
    /// The whole preview: the tone is estimated on it, so the part matches the preview.
    preview: Arc<RawImage>,
    frame: Crop,
    scale: f32,
}

struct DetailRendered {
    generation: u64,
    key: Edits,
    window: Crop,
    scale: f32,
    image: ColorImage,
}

/// Renders the sharp part on its own thread; only the newest request is rendered.
struct DetailRenderer {
    tx: Sender<DetailJob>,
    rx: Receiver<DetailRendered>,
}

impl DetailRenderer {
    fn new(ctx: &Context) -> Self {
        let (tx, jobs) = channel::<DetailJob>();
        let (done, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let out = crate::trace::timed("editor detail render", || detail_job(job));
                if let Some(out) = out {
                    if done.send(out).is_err() {
                        break;
                    }
                }
                ctx.request_repaint();
            }
        });
        Self { tx, rx }
    }
}

/// Heals, tones and sharpens the part `job.frame` of the level. The spots are in file
/// coordinates and move into the part's.
fn detail_job(job: DetailJob) -> Option<DetailRendered> {
    let (w, h) = (job.level.width, job.level.height);
    let (x0, y0, cw, ch) = preview::region_rect(w, h, job.frame)?;
    let mut part = preview::region(&job.level, job.frame)?;
    let spots: Vec<Spot> = job
        .key
        .spots
        .iter()
        .filter_map(|s| {
            let (cx, cy) = (s.x * w as f32 - x0 as f32, s.y * h as f32 - y0 as f32);
            let r = s.r * w as f32;
            let inside = cx > -r && cy > -r && cx < cw as f32 + r && cy < ch as f32 + r;
            inside.then(|| Spot {
                x: cx / cw as f32,
                y: cy / ch as f32,
                r: r / cw as f32,
            })
        })
        .collect();
    heal(&mut part, &spots);
    if let Some(tone) = tone_for(&job.preview, &job.key) {
        tone.apply(&mut part);
    }
    clarity(&mut part, job.key.clarity);
    sharpen(&mut part, job.key.sharpen.scaled(job.scale));
    Some(DetailRendered {
        generation: job.generation,
        key: job.key,
        window: [
            x0 as f32 / w as f32,
            y0 as f32 / h as f32,
            (x0 + cw) as f32 / w as f32,
            (y0 + ch) as f32 / h as f32,
        ],
        scale: job.scale,
        image: to_color_image(&part, u32::MAX),
    })
}

/// The level to draw a view from that shows `need` screen pixels per file pixel: the smallest
/// level that still has that many (the biggest when none has). `widths` are the levels' widths,
/// biggest first. `None` when the preview (`preview_scale` pixels per file pixel) is sharp enough.
pub fn pick_level(widths: &[u32], file_width: u32, need: f32, preview_scale: f32) -> Option<usize> {
    if widths.is_empty() || need <= preview_scale {
        return None;
    }
    let mut pick = 0;
    for (i, &w) in widths.iter().enumerate() {
        if w as f32 / file_width.max(1) as f32 >= need {
            pick = i;
        }
    }
    Some(pick)
}

/// `visible` grown by `margin` of its size on every side, kept inside the picture.
pub fn expand(visible: Crop, margin: f32) -> Crop {
    let (w, h) = (visible[2] - visible[0], visible[3] - visible[1]);
    [
        (visible[0] - w * margin).max(0.0),
        (visible[1] - h * margin).max(0.0),
        (visible[2] + w * margin).min(1.0),
        (visible[3] + h * margin).min(1.0),
    ]
}

/// `inner` lies within `outer` (a pixel of slack).
pub fn covers(outer: Crop, inner: Crop) -> bool {
    const EPS: f32 = 1e-4;
    outer[0] <= inner[0] + EPS
        && outer[1] <= inner[1] + EPS
        && outer[2] >= inner[2] - EPS
        && outer[3] >= inner[3] - EPS
}

impl EditorState {
    /// The decoded file arrived: the preview, and the levels for the sharp part.
    pub fn loaded_from(&mut self, l: Loaded) {
        self.loaded(l.preview, l.size);
        self.levels = l.levels;
        self.limited = l.limited;
    }

    /// Takes in the sharp parts the render thread finished.
    fn take_detail(&mut self, ctx: &Context) {
        let Some(renderer) = &self.detail_renderer else {
            return;
        };
        while let Ok(out) = renderer.rx.try_recv() {
            if out.generation != self.generation {
                continue;
            }
            self.detail = Some(Detail {
                tex: ctx.load_texture(
                    format!("editor-detail:{}", self.rel),
                    out.image,
                    Default::default(),
                ),
                window: out.window,
                key: out.key,
                generation: out.generation,
                scale: out.scale,
            });
            self.detail_asked = None;
        }
    }

    /// Asks for a sharp part when the view needs one: `visible` is the part of the picture on
    /// screen (normalized in the file) and `need` the screen pixels per file pixel. The part is
    /// rendered once the view and the edits have rested for a moment.
    pub fn update_detail(&mut self, ctx: &Context, visible: Crop, need: f32) {
        self.take_detail(ctx);
        let widths: Vec<u32> = self.levels.iter().map(|l| l.width).collect();
        let usable = self.base.is_some() && self.edits.turns.is_multiple_of(4);
        let pick = usable
            .then(|| pick_level(&widths, self.file_size[0], need, self.scale))
            .flatten();
        let Some(i) = pick else {
            self.detail = None;
            self.detail_asked = None;
            return;
        };
        let scale = widths[i] as f32 / self.file_size[0].max(1) as f32;
        let key = self.edits.render_key();
        let covered = |w: Crop, s: f32| covers(w, visible) && s >= scale * 0.999;
        if self.detail.as_ref().is_some_and(|d| {
            d.key == key && d.generation == self.generation && covered(d.window, d.scale)
        }) {
            return;
        }
        if self
            .detail_asked
            .as_ref()
            .is_some_and(|(k, w, g, s)| *k == key && *g == self.generation && covered(*w, *s))
        {
            return;
        }
        // The sharp part must show what the preview shows, so wait for the preview.
        if self.shown.as_ref() != Some(&key) || self.grab.is_some() || self.painting {
            return;
        }
        // Marking a spot is one click, not a drag of a slider: the sharp part is made at once
        // (the one on screen stays until then, see `detail_to_draw`).
        let quick = self
            .detail
            .as_ref()
            .is_some_and(|d| d.generation == self.generation && spots_only_change(&d.key, &key));
        let quantized = visible.map(|v| (v * 200.0) as i32);
        let now = ctx.input(|i| i.time);
        if !quick {
            match &self.detail_quiet {
                Some((k, q, since)) if *k == key && *q == quantized => {
                    if now - since < DETAIL_DELAY {
                        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                            DETAIL_DELAY - (now - since),
                        ));
                        return;
                    }
                }
                _ => {
                    self.detail_quiet = Some((key, quantized, now));
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(DETAIL_DELAY));
                    return;
                }
            }
        }
        let frame = expand(visible, DETAIL_MARGIN);
        let level = self.levels[i].clone();
        let Some((_, _, cw, ch)) = preview::region_rect(level.width, level.height, frame) else {
            return;
        };
        if cw > DETAIL_MAX_SIDE || ch > DETAIL_MAX_SIDE {
            return;
        }
        let Some(preview) = self.base.clone() else {
            return;
        };
        let job = DetailJob {
            generation: self.generation,
            key: key.clone(),
            level,
            preview,
            frame,
            scale,
        };
        let renderer = self
            .detail_renderer
            .get_or_insert_with(|| DetailRenderer::new(ctx));
        if renderer.tx.send(job).is_ok() {
            self.detail_asked = Some((key, frame, self.generation, scale));
        }
    }

    /// Call after drawing the picture in the pane `rect`, which shows the part `window` of it
    /// (view space; the file's, while it is not turned): keeps the sharp part up to date for the
    /// zoom and paints it over the preview.
    pub fn show_detail(&mut self, ui: &egui::Ui, rect: egui::Rect, window: Crop) {
        let ctx = ui.ctx();
        let file_w = self.file_size[0].max(1) as f32;
        let points_per_pixel = rect.width() / (window[2] - window[0]).max(1e-6) / file_w;
        self.pixel_zoom = points_per_pixel;
        let visible = crate::view::visible(window);
        self.update_detail(ctx, visible, points_per_pixel * ctx.pixels_per_point());
        let Some(d) = self.detail_to_draw() else {
            return;
        };
        let dst = egui::Rect::from_min_max(
            crate::crop::to_screen(rect, window, [d.window[0], d.window[1]]),
            crate::crop::to_screen(rect, window, [d.window[2], d.window[3]]),
        );
        ui.painter_at(rect).image(
            d.tex.id(),
            dst,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    /// The sharp part is on screen (the zoom needed more than the preview has).
    pub fn has_detail(&self) -> bool {
        self.detail.is_some()
    }

    /// The sharp part to paint over the preview: the one made for the current edits, or, while a
    /// new one is on its way after only the dust spots changed, the previous one. Swapping the
    /// sharp part for the blurry preview and back on every click would make the picture flicker;
    /// after any other edit (a slider) the old part would show the wrong tone, so it is hidden.
    pub fn detail_to_draw(&self) -> Option<&Detail> {
        let key = self.edits.render_key();
        self.detail
            .as_ref()
            .filter(|d| d.generation == self.generation)
            .filter(|d| d.key == key || spots_only_change(&d.key, &key))
    }
}

/// `a` and `b` are the same edits but for the dust spots.
fn spots_only_change(a: &Edits, b: &Edits) -> bool {
    let bare = |e: &Edits| Edits {
        spots: vec![],
        ..e.clone()
    };
    a.spots != b.spots && bare(a) == bare(b)
}

/// Applies `edits` to the file on a thread, after backing the original up (see
/// [`edit::backup_original`]; an existing backup is kept). Answers with the image written.
pub fn apply_async(
    ctx: &Context,
    root: PathBuf,
    path: PathBuf,
    edits: Edits,
) -> Receiver<Result<RawImage, String>> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = edit::backup_original(&root, &path).and_then(|()| apply_file(&path, &edits));
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use latent_image_edit::heal::Spot;
    use latent_image_edit::tone::Adjust;
    use latent_image_edit::write_image;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("edit-state-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A 16-bit RGB file whose left half is dark and right half bright, with a bright speck.
    fn write_specked(path: &Path, w: u32, h: u32) {
        let mut data = vec![];
        for y in 0..h {
            for x in 0..w {
                let speck = (x as i64 - 20).pow(2) + (y as i64 - 15).pow(2) <= 4;
                let v = if speck {
                    65535u16
                } else if x < w / 2 {
                    10000
                } else {
                    40000
                };
                data.extend([v, v, v]);
            }
        }
        write_image(&RawImage::new(w, h, 3, data).unwrap(), path).unwrap();
    }

    #[test]
    fn the_level_is_the_smallest_one_that_has_enough_pixels() {
        let widths = [4200, 2100];
        let preview_scale = 2048.0 / 4200.0;
        // The preview is sharp enough: no sharp part.
        assert_eq!(pick_level(&widths, 4200, 0.3, preview_scale), None);
        assert_eq!(
            pick_level(&widths, 4200, preview_scale, preview_scale),
            None
        );
        // Between the halved level (0.5) and the full one (1.0).
        assert_eq!(pick_level(&widths, 4200, 0.5, preview_scale), Some(1));
        assert_eq!(pick_level(&widths, 4200, 0.51, preview_scale), Some(0));
        assert_eq!(pick_level(&widths, 4200, 1.0, preview_scale), Some(0));
        // Beyond 1:1 the full picture is all there is.
        assert_eq!(pick_level(&widths, 4200, 3.0, preview_scale), Some(0));
        assert_eq!(pick_level(&[], 4200, 1.0, preview_scale), None);
    }

    #[test]
    fn expanding_keeps_inside_the_picture_and_covering_is_checked() {
        let v = [0.4, 0.4, 0.6, 0.6];
        let e = expand(v, 0.25);
        assert!(covers(e, v) && !covers(v, e));
        assert_eq!(expand([0.0, 0.0, 0.2, 1.0], 0.25), [0.0, 0.0, 0.25, 1.0]);
        assert!(covers([0.0, 0.0, 0.25, 1.0], [0.05, 0.0, 0.2, 1.0]));
        assert!(!covers([0.0, 0.0, 0.25, 1.0], [0.1, 0.0, 0.3, 1.0]));
    }

    fn write_wide(path: &Path, w: u32, h: u32) {
        let mut data = vec![];
        for _ in 0..h {
            for x in 0..w {
                let v = (x as u64 * 60000 / w as u64) as u16;
                data.extend([v, v, v]);
            }
        }
        write_image(&RawImage::new(w, h, 3, data).unwrap(), path).unwrap();
    }

    #[test]
    fn decoding_keeps_a_pyramid_within_the_memory_cap() {
        let root = dir("pyramid");
        let big = root.join("wide.tiff");
        write_wide(&big, 4200, 100);
        let l = decode(&big).unwrap();
        assert_eq!(l.size, [4200, 100]);
        assert!(l.preview.width.max(l.preview.height) <= PREVIEW_SIDE);
        let widths: Vec<u32> = l.levels.iter().map(|l| l.width).collect();
        assert_eq!(
            widths,
            [4200, 2100],
            "full size and one halving above the preview"
        );
        assert!(!l.limited);
        // Over the cap only the preview is kept, and the loader says so.
        let l = decode_with_cap(&big, 1000).unwrap();
        assert!(l.levels.is_empty() && l.limited);
        // A picture no bigger than the preview needs no levels.
        let small = root.join("small.tiff");
        write_wide(&small, 400, 100);
        let l = decode(&small).unwrap();
        assert!(l.levels.is_empty() && !l.limited);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_sharp_part_matches_the_same_part_of_the_whole_render() {
        let (w, h) = (60u32, 40u32);
        let mut data = vec![];
        for y in 0..h {
            for x in 0..w {
                let v = (x * 900 + y * 300) as u16;
                data.extend([v, v / 2, v / 3]);
            }
        }
        let level = Arc::new(RawImage::new(w, h, 3, data).unwrap());
        let preview = Arc::new(preview::downscale(&level, 30));
        let edits = Edits {
            adjust: Adjust {
                exposure_ev: 0.5,
                ..Adjust::default()
            },
            ..Edits::default()
        };
        let job = DetailJob {
            generation: 1,
            key: edits.clone(),
            level: level.clone(),
            preview,
            frame: [0.25, 0.25, 0.75, 0.75],
            scale: 1.0,
        };
        let out = detail_job(job).unwrap();
        assert_eq!(out.window, [0.25, 0.25, 0.75, 0.75]);
        assert_eq!(out.image.size, [30, 20]);
        let whole = render(&level, &edits, 1.0);
        for (x, y) in [(0, 0), (29, 19), (10, 5)] {
            assert_eq!(
                out.image.pixels[y * 30 + x],
                whole.pixels[(y + 10) * 60 + x + 15],
                "({x}, {y})"
            );
        }
    }

    #[test]
    fn dust_spots_move_into_the_sharp_part() {
        let (w, h) = (60u32, 40u32);
        let mut data = vec![20000u16; (w * h * 3) as usize];
        for c in 0..3 {
            data[((20 * w + 30) * 3) as usize + c] = 65535;
        }
        let level = Arc::new(RawImage::new(w, h, 3, data).unwrap());
        let spot = Spot {
            x: 0.5,
            y: 0.5,
            r: 3.0 / 60.0,
        };
        let job = |spots: Vec<Spot>| DetailJob {
            generation: 1,
            key: Edits {
                spots,
                ..Edits::default()
            },
            level: level.clone(),
            preview: level.clone(),
            frame: [0.25, 0.25, 0.75, 0.75],
            scale: 1.0,
        };
        // The speck is at (15, 10) of the part (origin 15, 10).
        let flat = detail_job(job(vec![spot])).unwrap().image;
        let speck = detail_job(job(vec![])).unwrap().image;
        assert_eq!(flat.pixels[10 * 30 + 15], flat.pixels[0], "healed");
        assert_ne!(speck.pixels[10 * 30 + 15], speck.pixels[0], "still there");
        // A spot outside the part does not touch it.
        let far = Spot {
            x: 0.05,
            y: 0.05,
            r: 0.02,
        };
        let same = detail_job(job(vec![far])).unwrap().image;
        assert_eq!(same.pixels, speck.pixels);
    }

    #[test]
    fn zooming_in_renders_a_sharp_part_and_zooming_out_drops_it() {
        let ctx = Context::default();
        let root = dir("detail");
        let file = root.join("wide.tiff");
        write_wide(&file, 4200, 100);
        let mut st = EditorState::new(&root, "wide.tiff");
        st.loaded_from(decode(&file).unwrap());
        let visible = [0.0, 0.0, 0.2, 1.0];
        let mut t = 0.0;
        let mut frame = |st: &mut EditorState, visible, need| {
            t += 0.05;
            let input = egui::RawInput {
                time: Some(t),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                st.refresh(ctx);
                st.update_detail(ctx, visible, need);
            });
        };
        // Fitted: the preview is sharp enough.
        for _ in 0..40 {
            frame(&mut st, [0.0, 0.0, 1.0, 1.0], 0.3);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!st.has_detail());
        // One file pixel per screen pixel on the left fifth.
        for _ in 0..400 {
            frame(&mut st, visible, 1.0);
            if st.has_detail() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let d = st.detail.as_ref().expect("a sharp part was rendered");
        assert!(covers(d.window, visible));
        assert_eq!(d.scale, 1.0, "from the full-size picture");
        // Zoomed out again, the preview will do.
        frame(&mut st, [0.0, 0.0, 1.0, 1.0], 0.3);
        assert!(!st.has_detail());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_new_spot_keeps_the_sharp_part_until_the_new_one_is_ready() {
        let ctx = Context::default();
        let root = dir("detail-spots");
        let file = root.join("wide.tiff");
        write_wide(&file, 4200, 100);
        let mut st = EditorState::new(&root, "wide.tiff");
        st.loaded_from(decode(&file).unwrap());
        let visible = [0.0, 0.0, 0.2, 1.0];
        let mut t = 0.0;
        let mut frame = |st: &mut EditorState, step: f64| {
            t += step;
            let input = egui::RawInput {
                time: Some(t),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                st.refresh(ctx);
                st.update_detail(ctx, visible, 1.0);
            });
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        for _ in 0..400 {
            frame(&mut st, 0.05);
            if st.has_detail() {
                break;
            }
        }
        assert!(st.detail_to_draw().is_some());
        let first_key = st.detail.as_ref().unwrap().key.clone();

        // A spot: the sharp part on screen stays, and the new one comes without the slider delay
        // (the clock advances by a millisecond a frame, far less than `DETAIL_DELAY`).
        st.edits.spots.push(Spot {
            x: 0.05,
            y: 0.5,
            r: 0.004,
        });
        assert!(
            st.detail_to_draw().is_some(),
            "kept while the new one is made"
        );
        for _ in 0..400 {
            frame(&mut st, 0.001);
            if st.detail.as_ref().is_some_and(|d| d.key != first_key) {
                break;
            }
        }
        let d = st.detail_to_draw().expect("the new sharp part");
        assert_eq!(d.key.spots.len(), 1, "it shows the spot");

        // A slider: the old tone would be wrong, so nothing is drawn until the new part is ready.
        st.edits.adjust.exposure_ev = 0.5;
        assert!(st.detail_to_draw().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn only_a_change_of_the_spots_alone_counts_as_a_spots_change() {
        let a = Edits::default();
        let spot = Spot {
            x: 0.5,
            y: 0.5,
            r: 0.01,
        };
        let b = Edits {
            spots: vec![spot],
            ..Edits::default()
        };
        assert!(spots_only_change(&a, &b) && spots_only_change(&b, &a));
        assert!(!spots_only_change(&a, &a));
        let c = Edits {
            negative: true,
            ..b.clone()
        };
        assert!(
            !spots_only_change(&a, &c),
            "a slider as well is not a spots change"
        );
    }

    /// A state with a small flat preview loaded, as if the file (40 x 20) had been decoded.
    fn loaded_state(name: &str) -> (EditorState, PathBuf) {
        let root = dir(name);
        let mut st = EditorState::new(&root, "images/x.tiff");
        st.loaded(
            RawImage::new(20, 10, 3, vec![20000; 600]).unwrap(),
            [40, 20],
        );
        (st, root)
    }

    /// Refreshes until the render thread has delivered what was asked for.
    fn settle(st: &mut EditorState, ctx: &Context) {
        for _ in 0..400 {
            st.refresh(ctx);
            if !st.rendering() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the preview was never rendered");
    }

    #[test]
    fn a_burst_of_edits_ends_with_the_newest_preview() {
        let (mut st, root) = loaded_state("burst");
        let ctx = Context::default();
        for step in 1..=8 {
            st.edits.adjust.exposure_ev = step as f32 * 0.1;
            st.refresh(&ctx);
        }
        settle(&mut st, &ctx);
        let want = st.edits.render_key();
        assert_eq!(
            st.shown.as_ref(),
            Some(&want),
            "the last request is what is shown"
        );
        assert!(st.tex.is_some());
        // A late answer for an older picture is ignored.
        let generation = st.generation;
        st.loaded(RawImage::new(2, 2, 1, vec![1000; 4]).unwrap(), [4, 4]);
        assert_eq!(st.generation, generation + 1);
        assert!(st.shown.is_none() && !st.rendering());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_preview_shows_healed_spots_and_caches_them() {
        let (mut st, root) = loaded_state("dust-preview");
        let ctx = Context::default();
        let mut base = RawImage::new(60, 40, 3, vec![20000; 60 * 40 * 3]).unwrap();
        for c in 0..3 {
            base.data[(20 * 60 + 30) * 3 + c] = 65535;
        }
        st.loaded(base, [60, 40]);
        st.edits.spots.push(Spot {
            x: 0.5,
            y: 0.5,
            r: 3.0 / 60.0,
        });
        let spots = st.edits.spots.clone();
        let base = st.base.clone().unwrap();
        let mut cache = HealCache::default();
        let healed = cache.base(&base, &spots) as *const RawImage;
        assert_eq!(unsafe { &*healed }.data[(20 * 60 + 30) * 3], 20000);
        assert!(
            std::ptr::eq(healed, cache.base(&base, &spots)),
            "the healed copy is reused for the same spots"
        );
        // Through the render thread the same preview ends up in the texture.
        settle(&mut st, &ctx);
        assert!(st.tex.is_some() && !st.rendering());
        assert_eq!(st.base.as_ref().unwrap().data[(20 * 60 + 30) * 3], 65535);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_file_operations_back_up_apply_and_restore() {
        let root = dir("ops");
        let file = root.join("x.tiff");
        write_specked(&file, 40, 30);
        let before = std::fs::read(&file).unwrap();
        assert_eq!(run_op(&root, &file, EditorOp::Backup), Ok(None));
        assert!(edit::has_original(&root, &file));
        let edits = Edits {
            turns: 1,
            ..Edits::default()
        };
        let op = EditorOp::Apply {
            edits,
            backup_first: false,
        };
        assert_eq!(op.name(), "Apply");
        let img = run_op(&root, &file, op).unwrap().unwrap();
        assert_eq!((img.width, img.height), (30, 40));
        assert_ne!(std::fs::read(&file).unwrap(), before);
        assert_eq!(run_op(&root, &file, EditorOp::Restore), Ok(None));
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert!(!edit::has_original(&root, &file));
    }

    #[test]
    fn apply_can_back_up_first() {
        let root = dir("ops-backup-first");
        let file = root.join("y.tiff");
        write_specked(&file, 40, 30);
        let op = EditorOp::Apply {
            edits: Edits {
                turns: 2,
                ..Edits::default()
            },
            backup_first: true,
        };
        run_op(&root, &file, op).unwrap();
        assert!(edit::has_original(&root, &file));
    }

    #[test]
    fn nothing_is_pending_until_something_changes() {
        assert!(Edits::default().is_neutral());
        let changed = [
            Edits {
                negative: true,
                ..Edits::default()
            },
            Edits {
                adjust: Adjust {
                    exposure_ev: 0.5,
                    ..Adjust::default()
                },
                ..Edits::default()
            },
            Edits {
                crop: Some([0.1, 0.1, 0.9, 0.9]),
                ..Edits::default()
            },
            Edits {
                spots: vec![Spot {
                    x: 0.5,
                    y: 0.5,
                    r: 0.01,
                }],
                ..Edits::default()
            },
            Edits {
                eight_bit: true,
                ..Edits::default()
            },
        ];
        for e in changed {
            assert!(!e.is_neutral(), "{e:?}");
        }
        let full = Edits {
            crop: Some(FULL),
            ..Edits::default()
        };
        assert!(full.is_neutral(), "a full-size crop changes nothing");
    }

    #[test]
    fn only_tiff_png_and_jpeg_are_editable() {
        for ok in ["a.tif", "a.TIFF", "a.png", "a.jpg", "a.jpeg"] {
            assert!(is_editable(Path::new(ok)), "{ok}");
        }
        for no in ["a.webp", "a.gif", "a"] {
            assert!(!is_editable(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn dragging_the_crop_of_a_positive_does_not_re_render_the_picture() {
        let a = Edits::default();
        let b = Edits {
            crop: Some([0.2, 0.2, 0.8, 0.8]),
            ..Edits::default()
        };
        assert_eq!(a.render_key(), b.render_key());
        let neg = |c| Edits {
            negative: true,
            crop: c,
            ..Edits::default()
        };
        assert_ne!(
            neg(None).render_key(),
            neg(Some([0.2, 0.2, 0.8, 0.8])).render_key()
        );
    }

    #[test]
    fn apply_heals_marked_dust_at_full_resolution_and_keeps_16_bits() {
        let root = dir("dust-apply");
        let file = root.join("x.tiff");
        write_specked(&file, 80, 40);
        let edits = Edits {
            spots: vec![Spot {
                x: 20.0 / 80.0,
                y: 15.0 / 40.0,
                r: 3.0 / 80.0,
            }],
            ..Edits::default()
        };
        let out = apply_file(&file, &edits).unwrap();
        let at = |img: &RawImage, x: usize, y: usize| img.data[(y * 80 + x) * 3] as i32;
        assert!((at(&out, 20, 15) - 10000).abs() < 100, "speck is gone");
        assert_eq!(at(&out, 60, 15), 40000, "16-bit values elsewhere are kept");
        assert_eq!(read_image(&file).unwrap(), out, "written to the file");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn applying_crops_the_file_and_a_degenerate_crop_keeps_one_pixel() {
        let root = dir("crop-apply");
        let file = root.join("x.tiff");
        write_specked(&file, 80, 40);
        let edits = Edits {
            crop: Some([0.5, 0.0, 1.0, 0.5]),
            ..Edits::default()
        };
        let out = apply_file(&file, &edits).unwrap();
        assert_eq!((out.width, out.height), (40, 20));
        assert_eq!((read_image(&file).unwrap().width, out.data[0]), (40, 40000));
        let thin = Edits {
            crop: Some([0.5, 0.5, 0.5, 0.9]),
            ..Edits::default()
        };
        let out = apply_file(&file, &thin).unwrap();
        assert_eq!(out.width, 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn apply_async_backs_the_original_up_first() {
        let root = dir("apply-async");
        let file = root.join("x.tiff");
        write_specked(&file, 80, 40);
        let before = read_image(&file).unwrap();
        let edits = Edits {
            crop: Some([0.0, 0.0, 0.5, 0.5]),
            ..Edits::default()
        };
        let ctx = Context::default();
        let rx = apply_async(&ctx, root.clone(), file.clone(), edits);
        let out = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap()
            .unwrap();
        assert_eq!(out.width, 40);
        assert!(edit::has_original(&root, &file));
        let saved = root.join(".originals").join("x.tiff");
        assert_eq!(read_image(&saved).unwrap(), before);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn loading_sets_the_scale_and_reloading_clears_the_edits() {
        let root = dir("state");
        let mut st = EditorState::new(&root, "x.tiff");
        let img = RawImage::new(100, 50, 3, vec![0; 100 * 50 * 3]).unwrap();
        st.loaded(img, [400, 200]);
        assert_eq!(st.scale, 0.25);
        st.edits.negative = true;
        assert!(st.dirty());
        st.reset_for_reload(&root);
        assert!(!st.dirty() && st.base.is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn picking_a_gray_spot_cancels_the_cast_and_gray_pictures_have_none() {
        // Warm cast everywhere; the left half is the "gray card".
        let (w, h) = (40u32, 20u32);
        let mut data = vec![];
        for _y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 20000.0 } else { 40000.0 };
                data.extend([(v * 1.15) as u16, v as u16, (v * 0.87) as u16]);
            }
        }
        let base = RawImage::new(w, h, 3, data).unwrap();
        let edits = Edits::default();
        let (warmth, tint) = neutral_gray(&base, &edits, 0.25, 0.5).unwrap();
        assert!(warmth < 0.0, "the warm cast is cooled");
        let adjusted = Edits {
            adjust: Adjust {
                warmth,
                tint,
                ..Adjust::default()
            },
            ..Edits::default()
        };
        let mut px = RawImage::new(1, 1, 3, vec![23000, 20000, 17400]).unwrap();
        Tone::estimate(&px, adjusted.tone()).apply(&mut px);
        let spread = px.data.iter().max().unwrap() - px.data.iter().min().unwrap();
        assert!(spread < 400, "{:?}", px.data);
        let gray = RawImage::new(2, 2, 1, vec![100; 4]).unwrap();
        assert_eq!(neutral_gray(&gray, &edits, 0.5, 0.5), None);
    }

    #[test]
    fn a_rendered_preview_has_markers() {
        let ctx = Context::default();
        let root = dir("markers");
        let file = root.join("x.tiff");
        write_specked(&file, 80, 40);
        let mut st = EditorState::new(&root, "x.tiff");
        let loaded = load_async(&ctx, file)
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap()
            .unwrap();
        st.loaded_from(loaded);
        for _ in 0..200 {
            st.refresh(&ctx);
            if st.markers.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let m = st.markers.expect("rendered");
        assert!(m.black.0 < 0.5 && m.white.0 > 0.0);
        assert_eq!(m.gray, None, "the test picture is gray");
        std::fs::remove_dir_all(root).ok();
    }
}
