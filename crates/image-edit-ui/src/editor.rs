//! The image view of a saved scan: adjust, sharpen, rotate and crop with a live histogram,
//! modelled on the image view of latent.
//!
//! Edits only change the preview. **Apply** writes them to the file (at full resolution, same
//! format, 16 bit kept); the untouched original can be saved first with **Backup original**
//! (`.originals/…`, latent's layout, see [`crate::edit`]).

use crate::edit;
use crate::histogram::{marker_points, Histogram, Markers};
use crate::preview::{self, rotate_image, to_color_image};
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

/// A crop rectangle `[left, top, right, bottom]`, normalized to the picture.
pub type Crop = [f32; 4];
pub const FULL: Crop = [0.0, 0.0, 1.0, 1.0];

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

/// The edited preview (without the crop, which is an overlay until it is applied). `scale` is the
/// preview's size relative to the file, so the sharpening radius matches the final result.
pub fn render(base: &RawImage, edits: &Edits, scale: f32) -> ColorImage {
    let mut img = base.clone();
    let tone = edits.tone();
    if !tone.is_identity() {
        // The negative base comes from the crop, as it will when applied.
        let reference = edits.crop.and_then(|c| preview::region(base, c));
        Tone::estimate(reference.as_ref().unwrap_or(base), tone).apply(&mut img);
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
    /// The pointer is down while marking dust or dragging the crop: the preview waits until the
    /// edit is complete.
    pub painting: bool,
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
            painting: false,
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
        self.shown = None;
        self.requested = None;
        self.generation += 1;
        self.busy = None;
        self.error = None;
    }

    /// Back to the file as it is (after an apply or a restore): the preview is decoded again.
    pub fn reset_for_reload(&mut self, root: &Path) {
        self.edits = Edits::default();
        self.base = None;
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
        if self.painting {
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

/// Decodes the file on a thread: the preview (at most [`PREVIEW_SIDE`]) and the file's size.
pub fn load_async(ctx: &Context, path: PathBuf) -> Receiver<Result<(RawImage, [u32; 2]), String>> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = read_image(&path)
            .map(|full| {
                let size = [full.width, full.height];
                (preview::downscale(&full, PREVIEW_SIDE), size)
            })
            .map_err(|e| format!("cannot read {}: {e}", path.display()));
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
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
        let (img, size) = load_async(&ctx, file)
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap()
            .unwrap();
        st.loaded(img, size);
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
