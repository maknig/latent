//! Negative conversion, automatic levels and manual image adjustments.
//!
//! A [`Tone`] is estimated from a reference image and then applied to any image, so a preview
//! can show a whole overview toned like one chosen frame will be.

use crate::Stage;
use crate::{RawImage, Result};

/// Percentile of the (thin-density) film base used as the white point of the negative.
const BASE_PERCENTILE: f64 = 0.995;
/// Percentiles clipped by the auto levels stretch.
const LEVELS_LOW: f64 = 0.001;
const LEVELS_HIGH: f64 = 0.998;
/// Ranges of the black and white point sliders; they keep at least `MIN_RANGE` between them.
const MAX_BLACK: f32 = 0.4;
const MIN_WHITE: f32 = 0.6;
const MIN_RANGE: f32 = 0.1;
/// Largest shift of the darks / whites sliders (fraction of the range at the peak of the bump).
const TONE_BUMP: f32 = 0.14;
/// Stops of gain per unit of the warmth / tint sliders.
const BALANCE_STOPS: f32 = 0.3;

/// Manual adjustments applied after the negative and auto levels step. The neutral value
/// ([`Adjust::default`]) leaves the image untouched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjust {
    /// Exposure change in stops (`-2.0..=2.0`).
    pub exposure_ev: f32,
    /// Black point as a fraction of the range (`0.0..=0.4`).
    pub black: f32,
    /// White point as a fraction of the range (`0.6..=1.0`).
    pub white: f32,
    /// Midtone gamma; above 1 brightens the middle.
    pub gamma: f32,
    /// Warm (positive: more red, less blue) or cool (`-1.0..=1.0`); colour images only.
    pub warmth: f32,
    /// Magenta (negative) to green (positive) (`-1.0..=1.0`); colour images only.
    pub tint: f32,
    /// Steepens (positive) or flattens (negative) the curve around mid-grey (`-1.0..=1.0`).
    pub contrast: f32,
    /// Brightens (positive) or darkens (negative) the shadows without moving black
    /// (`-1.0..=1.0`).
    pub darks: f32,
    /// Brightens (positive) or darkens (negative) the highlights without moving white
    /// (`-1.0..=1.0`).
    pub whites: f32,
    /// Colour intensity: -1 gives gray, 0 leaves it, positive saturates; colour images only
    /// (`-1.0..=1.0`).
    pub saturation: f32,
}

impl Default for Adjust {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            black: 0.0,
            white: 1.0,
            gamma: 1.0,
            warmth: 0.0,
            tint: 0.0,
            contrast: 0.0,
            darks: 0.0,
            whites: 0.0,
            saturation: 0.0,
        }
    }
}

impl Adjust {
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// The saturation slider: every channel moves away from (or toward) the pixel's luma.
    fn saturate(&self, img: &mut RawImage) {
        if self.saturation == 0.0 || img.channels != 3 {
            return;
        }
        let k = 1.0 + self.saturation;
        for px in img.data.as_chunks_mut::<3>().0 {
            let [r, g, b] = [px[0] as f32, px[1] as f32, px[2] as f32];
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            for (v, c) in px.iter_mut().zip([r, g, b]) {
                *v = (luma + (c - luma) * k).round().clamp(0.0, 65535.0) as u16;
            }
        }
    }

    /// Gain of colour channel `c` out of `n` for the warmth and tint sliders.
    fn channel_gain(&self, c: usize, n: usize) -> f32 {
        if n != 3 {
            return 1.0;
        }
        let stops = match c {
            0 => self.warmth,
            1 => self.tint,
            _ => -self.warmth,
        };
        (BALANCE_STOPS * stops).exp2()
    }

    /// Warmth and tint that turn the colour `rgb` (values after the negative step) neutral.
    /// The gains are applied before the same monotonic curve for every channel, so equal channels
    /// going in come out equal. Red and blue gains multiply to one, which keeps the geometric
    /// mean of red and blue (the brightness) and moves green onto it. Limited to the sliders'
    /// range; a black channel gives no answer worth acting on and leaves the sliders at 0.
    pub fn balance_for(rgb: [f32; 3]) -> (f32, f32) {
        let [r, g, b] = rgb;
        if r < 1.0 || g < 1.0 || b < 1.0 {
            return (0.0, 0.0);
        }
        let warmth = 0.5 * (b / r).log2() / BALANCE_STOPS;
        let tint = (0.5 * (r * b).log2() - g.log2()) / BALANCE_STOPS;
        (warmth.clamp(-1.0, 1.0), tint.clamp(-1.0, 1.0))
    }

    /// Maps a value in `0..=1`: exposure and balance gain, levels, gamma, then the tone
    /// shaping (darks and whites, contrast). Black and white map to themselves.
    fn map(&self, x: f32, gain: f32) -> f32 {
        let x = x * self.exposure_ev.exp2() * gain;
        let range = (self.white - self.black).max(1e-3);
        let x = ((x - self.black) / range)
            .clamp(0.0, 1.0)
            .powf(1.0 / self.gamma.max(1e-3));
        self.shape(x)
    }

    /// Darks and whites move the shadows and the highlights by smooth bumps that vanish at
    /// black and white; contrast is a cubic S-curve around mid-grey. The strengths keep the
    /// curve monotonic for every slider position.
    fn shape(&self, x: f32) -> f32 {
        if self.darks == 0.0 && self.whites == 0.0 && self.contrast == 0.0 {
            return x;
        }
        // Bumps peaking at 1/3 and 2/3 with height 1.
        let shadows = 27.0 / 4.0 * x * (1.0 - x) * (1.0 - x);
        let highlights = 27.0 / 4.0 * x * x * (1.0 - x);
        let x = (x + TONE_BUMP * (self.darks * shadows + self.whites * highlights)).clamp(0.0, 1.0);
        (x + self.contrast * x * (1.0 - x) * (2.0 * x - 1.0)).clamp(0.0, 1.0)
    }
}

impl Adjust {
    /// Black and white point that stretch `reference` (a frame's pixels) to the full range:
    /// the 0.1th and 99.8th percentile of all channels pooled, measured after the negative
    /// step when `negative` is set, limited to the ranges of the sliders. Everything else stays
    /// neutral. A flat reference gives the neutral adjustment.
    pub fn auto_levels(reference: &RawImage, negative: bool) -> Self {
        let mut img = reference.clone();
        if negative {
            let only_negative = ToneOptions {
                negative: true,
                ..ToneOptions::default()
            };
            Tone::estimate(&img, only_negative).apply(&mut img);
        }
        let pooled = to_gray(&img);
        let hist = histogram(pooled.data.iter().copied());
        let (lo, hi) = (
            percentile(&hist, LEVELS_LOW),
            percentile(&hist, LEVELS_HIGH),
        );
        if hi <= lo {
            return Self::default();
        }
        let max = u16::MAX as f32;
        let black = (lo as f32 / max).clamp(0.0, MAX_BLACK);
        let white = (hi as f32 / max)
            .clamp(MIN_WHITE, 1.0)
            .max(black + MIN_RANGE)
            .min(1.0);
        Self {
            black,
            white,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ToneOptions {
    /// Invert and balance each channel against the film base (removes the orange mask).
    pub negative: bool,
    /// Stretch each channel to the full range.
    pub auto_levels: bool,
    /// Manual adjustments on top.
    pub adjust: Adjust,
}

impl ToneOptions {
    pub fn is_identity(&self) -> bool {
        !self.negative && !self.auto_levels && self.adjust.is_neutral()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Channel {
    /// Film base value of the negative; `None` leaves the channel uninverted.
    base: Option<f32>,
    lo: f32,
    hi: f32,
}

impl Channel {
    /// Negative and auto levels, in sample units (`0.0..=65535.0`).
    fn base_map(&self, v: u16) -> f32 {
        let max = u16::MAX as f32;
        let mut x = v as f32;
        if let Some(base) = self.base {
            x = (1.0 - x / base).clamp(0.0, 1.0) * max;
        }
        if self.hi > self.lo {
            x = ((x - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0) * max;
        }
        x
    }

    fn map(&self, v: u16) -> u16 {
        self.base_map(v).round() as u16
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tone {
    channels: Vec<Channel>,
    adjust: Adjust,
}

impl Tone {
    /// Estimates the mapping for `opts` from the samples of `reference`.
    pub fn estimate(reference: &RawImage, opts: ToneOptions) -> Self {
        let n = reference.channels as usize;
        let channels = (0..n)
            .map(|c| {
                let samples = || reference.data.iter().skip(c).step_by(n).copied();
                let mut ch = Channel {
                    base: None,
                    lo: 0.0,
                    hi: 0.0,
                };
                if opts.negative {
                    let base = percentile(&histogram(samples()), BASE_PERCENTILE);
                    ch.base = Some(base.max(1) as f32);
                }
                if opts.auto_levels {
                    let hist = histogram(samples().map(|v| ch.map(v)));
                    ch.lo = percentile(&hist, LEVELS_LOW) as f32;
                    ch.hi = percentile(&hist, LEVELS_HIGH) as f32;
                }
                ch
            })
            .collect();
        Self {
            channels,
            adjust: opts.adjust,
        }
    }

    /// The same estimated negative base and levels with other manual adjustments, so a slider
    /// can move without estimating again.
    pub fn with_adjust(&self, adjust: Adjust) -> Self {
        Self {
            adjust,
            ..self.clone()
        }
    }

    /// Applies the mapping in place. The image needs as many channels as the reference.
    pub fn apply(&self, img: &mut RawImage) {
        let n = self.channels.len();
        if n != img.channels as usize {
            return;
        }
        // One lookup table per channel: far cheaper than the maths per sample on 100 MP scans.
        let luts: Vec<Vec<u16>> = self
            .channels
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                let gain = self.adjust.channel_gain(c, n);
                (0..=u16::MAX)
                    .map(|v| {
                        let x = ch.base_map(v);
                        if self.adjust.is_neutral() {
                            x.round() as u16
                        } else {
                            (self.adjust.map(x / u16::MAX as f32, gain) * u16::MAX as f32).round()
                                as u16
                        }
                    })
                    .collect()
            })
            .collect();
        for px in img.data.chunks_exact_mut(n) {
            for (v, lut) in px.iter_mut().zip(&luts) {
                *v = lut[*v as usize];
            }
        }
        self.adjust.saturate(img);
    }
}

/// Estimates the tone from the image itself and applies it.
pub struct ToneStage(pub ToneOptions);

impl Stage for ToneStage {
    fn apply(&self, mut img: RawImage) -> Result<RawImage> {
        if !self.0.is_identity() {
            Tone::estimate(&img, self.0).apply(&mut img);
        }
        Ok(img)
    }
}

/// Luma (Rec. 709 weights) of an RGB image; gray images are returned unchanged.
pub fn to_gray(img: &RawImage) -> RawImage {
    if img.channels != 3 {
        return img.clone();
    }
    let data = img
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| {
            (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).round() as u16
        })
        .collect();
    RawImage {
        channels: 1,
        data,
        ..*img
    }
}

fn histogram(samples: impl Iterator<Item = u16>) -> Vec<u32> {
    let mut h = vec![0u32; 65536];
    for v in samples {
        h[v as usize] += 1;
    }
    h
}

/// Smallest value `v` such that at least the fraction `p` of the samples is `<= v`.
fn percentile(hist: &[u32], p: f64) -> u16 {
    let total: u64 = hist.iter().map(|&c| c as u64).sum();
    let target = (p * total as f64).ceil().max(1.0) as u64;
    let mut cum = 0u64;
    for (v, &c) in hist.iter().enumerate() {
        cum += c as u64;
        if cum >= target {
            return v as u16;
        }
    }
    u16::MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(data: Vec<u16>) -> RawImage {
        RawImage::new(data.len() as u32 / 3, 1, 3, data).unwrap()
    }

    fn mean(img: &RawImage, c: usize) -> f64 {
        let v: Vec<f64> = img
            .data
            .iter()
            .skip(c)
            .step_by(3)
            .map(|&v| v as f64)
            .collect();
        v.iter().sum::<f64>() / v.len() as f64
    }

    #[test]
    fn percentile_picks_the_smallest_covering_value() {
        let h = histogram([1u16, 2, 3, 4, 5, 6, 7, 8, 9, 10].into_iter());
        assert_eq!(percentile(&h, 0.5), 5);
        assert_eq!(percentile(&h, 1.0), 10);
        assert_eq!(percentile(&h, 0.001), 1);
    }

    #[test]
    fn negative_removes_an_orange_mask() {
        // The same scene densities on a mask whose channels have different bases.
        let bases = [60000.0_f64, 45000.0, 30000.0];
        let mut data = vec![];
        for i in 0..=200 {
            let d = i as f64 / 250.0; // density 0 (clear base) .. 0.8
            for b in bases {
                data.push((b * (1.0 - d)).round() as u16);
            }
        }
        let img = rgb(data);
        let out = ToneStage(ToneOptions {
            negative: true,
            auto_levels: false,
            ..ToneOptions::default()
        })
        .apply(img)
        .unwrap();
        let (r, g, b) = (mean(&out, 0), mean(&out, 1), mean(&out, 2));
        assert!(
            (r - g).abs() / r < 0.02 && (g - b).abs() / r < 0.02,
            "{r} {g} {b}"
        );
        assert!(r > 5000.0, "scene should not collapse to black: {r}");
    }

    #[test]
    fn auto_levels_stretch_to_the_full_range() {
        let data: Vec<u16> = (0..1000).map(|i| 20000 + i * 10).collect();
        let img = RawImage::new(1000, 1, 1, data).unwrap();
        let out = ToneStage(ToneOptions {
            negative: false,
            auto_levels: true,
            ..ToneOptions::default()
        })
        .apply(img)
        .unwrap();
        let min = *out.data.iter().min().unwrap();
        let max = *out.data.iter().max().unwrap();
        assert!(min <= 1 && max >= 65534, "{min}..{max}");
        assert!(out.data.windows(2).all(|w| w[0] <= w[1]), "monotonic");
    }

    #[test]
    fn gray_negative_is_inverted_against_its_base() {
        let img = RawImage::new(3, 1, 1, vec![50000, 25000, 0]).unwrap();
        let out = ToneStage(ToneOptions {
            negative: true,
            auto_levels: false,
            ..ToneOptions::default()
        })
        .apply(img)
        .unwrap();
        assert!(out.data[0] < out.data[1] && out.data[1] < out.data[2]);
        assert_eq!(out.data[0], 0);
    }

    #[test]
    fn flat_image_does_not_blow_up() {
        let img = RawImage::new(4, 1, 1, vec![1234; 4]).unwrap();
        let out = ToneStage(ToneOptions {
            negative: true,
            auto_levels: true,
            ..ToneOptions::default()
        })
        .apply(img)
        .unwrap();
        assert_eq!(out.data.len(), 4);
    }

    #[test]
    fn identity_options_leave_the_image_alone() {
        let img = rgb(vec![1, 2, 3, 4, 5, 6]);
        let out = ToneStage(ToneOptions::default())
            .apply(img.clone())
            .unwrap();
        assert_eq!(out, img);
    }

    #[test]
    fn tone_from_a_sub_image_applies_to_the_whole() {
        let reference = RawImage::new(2, 1, 1, vec![1000, 2000]).unwrap();
        let tone = Tone::estimate(
            &reference,
            ToneOptions {
                negative: false,
                auto_levels: true,
                ..ToneOptions::default()
            },
        );
        let mut whole = RawImage::new(3, 1, 1, vec![0, 1000, 5000]).unwrap();
        tone.apply(&mut whole);
        assert_eq!(whole.data[0], 0);
        assert_eq!(whole.data[2], 65535);
    }

    #[test]
    fn gray_conversion_uses_luma_weights() {
        let g = to_gray(&rgb(vec![65535, 0, 0, 0, 65535, 0]));
        assert_eq!(g.channels, 1);
        assert!(g.data[1] > g.data[0]);
    }

    fn with(adjust: Adjust) -> ToneOptions {
        ToneOptions {
            adjust,
            ..ToneOptions::default()
        }
    }

    fn gray(v: Vec<u16>) -> RawImage {
        RawImage::new(v.len() as u32, 1, 1, v).unwrap()
    }

    #[test]
    fn a_neutral_adjust_is_the_identity() {
        assert!(Adjust::default().is_neutral());
        assert!(with(Adjust::default()).is_identity());
        let img = rgb(vec![1, 2, 3, 40000, 50000, 65535]);
        let out = ToneStage(with(Adjust::default()))
            .apply(img.clone())
            .unwrap();
        assert_eq!(out, img);
    }

    #[test]
    fn exposure_changes_by_stops() {
        let a = Adjust {
            exposure_ev: 1.0,
            ..Adjust::default()
        };
        let out = ToneStage(with(a))
            .apply(gray(vec![10000, 20000, 40000]))
            .unwrap();
        assert!((out.data[0] as i32 - 20000).abs() <= 1);
        assert!((out.data[1] as i32 - 40000).abs() <= 1);
        assert_eq!(out.data[2], 65535, "clamped at white");
        let dark = Adjust {
            exposure_ev: -1.0,
            ..Adjust::default()
        };
        let out = ToneStage(with(dark)).apply(gray(vec![40000])).unwrap();
        assert!((out.data[0] as i32 - 20000).abs() <= 1);
    }

    #[test]
    fn black_and_white_points_clip_and_rescale() {
        let a = Adjust {
            black: 0.2,
            white: 0.8,
            ..Adjust::default()
        };
        let max = 65535.0;
        let out = ToneStage(with(a))
            .apply(gray(vec![
                (0.1 * max) as u16,
                (0.5 * max) as u16,
                (0.9 * max) as u16,
            ]))
            .unwrap();
        assert_eq!(out.data[0], 0);
        assert!(
            (out.data[1] as f32 / max - 0.5).abs() < 0.01,
            "midpoint stays in the middle"
        );
        assert_eq!(out.data[2], 65535);
    }

    #[test]
    fn gamma_moves_the_midtones_only() {
        let bright = Adjust {
            gamma: 2.0,
            ..Adjust::default()
        };
        let out = ToneStage(with(bright))
            .apply(gray(vec![0, 16384, 65535]))
            .unwrap();
        assert_eq!(out.data[0], 0);
        assert!(
            out.data[1] > 16384 + 5000,
            "0.25 becomes about 0.5: {}",
            out.data[1]
        );
        assert_eq!(out.data[2], 65535);
        let same = Adjust {
            gamma: 1.0,
            ..Adjust::default()
        };
        let out = ToneStage(with(same)).apply(gray(vec![16384])).unwrap();
        assert_eq!(out.data[0], 16384);
    }

    #[test]
    fn warmth_shifts_red_against_blue_and_tint_moves_green() {
        let warm = Adjust {
            warmth: 1.0,
            ..Adjust::default()
        };
        let out = ToneStage(with(warm))
            .apply(rgb(vec![30000, 30000, 30000]))
            .unwrap();
        assert!(out.data[0] > 30000 && out.data[2] < 30000);
        assert_eq!(out.data[1], 30000, "green untouched by warmth");
        let green = Adjust {
            tint: 1.0,
            ..Adjust::default()
        };
        let out = ToneStage(with(green))
            .apply(rgb(vec![30000, 30000, 30000]))
            .unwrap();
        assert!(out.data[1] > 30000);
        assert_eq!((out.data[0], out.data[2]), (30000, 30000));
    }

    #[test]
    fn balance_for_cancels_a_colour_cast() {
        // A mild warm cast with a green tint.
        let (warmth, tint) = Adjust::balance_for([36000.0, 30000.0, 25000.0]);
        let a = Adjust {
            warmth,
            tint,
            ..Adjust::default()
        };
        let out = ToneStage(with(a))
            .apply(rgb(vec![36000, 30000, 25000]))
            .unwrap();
        let spread = out.data.iter().max().unwrap() - out.data.iter().min().unwrap();
        assert!(spread <= 2, "{:?}", out.data);
        let mean = |v: &[u16]| (v[0] as f32 * v[2] as f32).sqrt();
        assert!((mean(&out.data) - mean(&[36000, 30000, 25000])).abs() < 5.0);
    }

    #[test]
    fn balance_for_leaves_gray_alone_and_stays_in_range() {
        assert_eq!(Adjust::balance_for([20000.0; 3]), (0.0, 0.0));
        assert_eq!(Adjust::balance_for([0.0, 5000.0, 9000.0]), (0.0, 0.0));
        let (w, t) = Adjust::balance_for([60000.0, 1000.0, 2000.0]);
        assert!((-1.0..=1.0).contains(&w) && (-1.0..=1.0).contains(&t));
        assert_eq!(t, 1.0, "a strongly magenta pixel needs the most green");
    }

    #[test]
    fn colour_balance_is_ignored_for_gray() {
        let a = Adjust {
            warmth: 1.0,
            tint: -1.0,
            ..Adjust::default()
        };
        let out = ToneStage(with(a)).apply(gray(vec![30000])).unwrap();
        assert_eq!(out.data[0], 30000);
    }

    #[test]
    fn adjust_comes_after_negative_and_auto_levels() {
        let opts = ToneOptions {
            negative: true,
            auto_levels: true,
            adjust: Adjust {
                exposure_ev: 1.0,
                ..Adjust::default()
            },
        };
        let plain = ToneOptions {
            adjust: Adjust::default(),
            ..opts
        };
        let data: Vec<u16> = (0..1000).map(|i| 20000 + i * 40).collect();
        let a = ToneStage(plain).apply(gray(data.clone())).unwrap();
        let b = ToneStage(opts).apply(gray(data)).unwrap();
        for (x, y) in a.data.iter().zip(&b.data) {
            let want = ((*x as u32) * 2).min(65535) as i32;
            assert!((want - *y as i32).abs() <= 2, "{x} -> {y}, want {want}");
        }
    }

    #[test]
    fn the_lookup_table_matches_the_formula() {
        let a = Adjust {
            exposure_ev: 0.5,
            black: 0.05,
            white: 0.95,
            gamma: 1.3,
            warmth: 0.4,
            tint: -0.2,
            contrast: 0.3,
            darks: 0.5,
            whites: -0.4,
            saturation: 0.0,
        };
        let img = rgb((0..30).map(|i| i * 2000).collect());
        let out = ToneStage(with(a)).apply(img.clone()).unwrap();
        for (i, (src, got)) in img.data.iter().zip(&out.data).enumerate() {
            let gain = a.channel_gain(i % 3, 3);
            let want = (a.map(*src as f32 / 65535.0, gain) * 65535.0).round() as i32;
            assert!(
                (want - *got as i32).abs() <= 1,
                "sample {i}: {src} -> {got}, want {want}"
            );
        }
    }

    #[test]
    fn with_adjust_reuses_the_estimated_negative_base() {
        let reference = rgb(vec![60000, 45000, 30000, 30000, 22000, 15000]);
        let negative = ToneOptions {
            negative: true,
            ..ToneOptions::default()
        };
        let tone = Tone::estimate(&reference, negative);
        let a = Adjust {
            exposure_ev: 1.0,
            ..Adjust::default()
        };
        let moved = tone.with_adjust(a);
        let mut x = reference.clone();
        moved.apply(&mut x);
        let direct = Tone::estimate(
            &reference,
            ToneOptions {
                adjust: a,
                ..negative
            },
        );
        let mut y = reference;
        direct.apply(&mut y);
        assert_eq!(x, y);
    }

    #[test]
    fn auto_levels_find_the_ends_of_the_data_and_stretch_it() {
        let data: Vec<u16> = (0..1000).map(|i| 20000 + i * 40).collect();
        let img = gray(data);
        let a = Adjust::auto_levels(&img, false);
        assert!((a.black - 20040.0 / 65535.0).abs() < 0.002, "{a:?}");
        assert!((a.white - 59600.0 / 65535.0).abs() < 0.005, "{a:?}");
        assert_eq!(
            (a.exposure_ev, a.gamma, a.warmth, a.tint),
            (0.0, 1.0, 0.0, 0.0)
        );

        let out = ToneStage(with(a)).apply(img).unwrap();
        assert_eq!(*out.data.iter().min().unwrap(), 0);
        assert_eq!(*out.data.iter().max().unwrap(), 65535);
    }

    #[test]
    fn auto_levels_of_a_flat_reference_are_neutral() {
        let a = Adjust::auto_levels(&gray(vec![30000; 50]), false);
        assert!(a.is_neutral());
    }

    #[test]
    fn auto_levels_are_measured_after_the_negative_step() {
        let data: Vec<u16> = (0..1000).map(|i| 20000 + i * 40).collect();
        let img = gray(data);
        let plain = Adjust::auto_levels(&img, false);
        let neg = Adjust::auto_levels(&img, true);
        // Inverted against the film base, the densest part is the brightest.
        assert!(neg.black < 0.02, "{neg:?}");
        assert!(neg.white > 0.6 && neg.white < 0.75, "{neg:?}");
        assert_ne!(plain, neg);
    }

    #[test]
    fn auto_levels_stay_within_the_slider_ranges() {
        // A very narrow, bright range would ask for black above 40 % and white below 60 %.
        let a = Adjust::auto_levels(&gray((0..500).map(|i| 40000 + i).collect()), false);
        assert!(a.black <= MAX_BLACK + 1e-6, "{a:?}");
        assert!(
            a.white >= MIN_WHITE - 1e-6 && a.white >= a.black + MIN_RANGE - 1e-6,
            "{a:?}"
        );
        assert!(a.white <= 1.0);
    }

    #[test]
    fn auto_levels_pool_the_colour_channels() {
        let mut data = vec![];
        for i in 0..500u32 {
            let v = (10000 + i * 90) as u16;
            data.extend([v, v / 2, v / 3]);
        }
        let a = Adjust::auto_levels(&rgb(data), false);
        // One pair for all channels, derived from the luma of the data.
        assert!(a.black > 0.0 && a.white > a.black);
    }

    /// The output (0..=65535) of a gray ramp through `adjust`.
    fn ramp(adjust: Adjust, points: &[u16]) -> Vec<i32> {
        let out = ToneStage(with(adjust))
            .apply(gray(points.to_vec()))
            .unwrap();
        out.data.iter().map(|&v| v as i32).collect()
    }

    fn at(adjust: Adjust, fraction: f32) -> f32 {
        ramp(adjust, &[(fraction * 65535.0).round() as u16])[0] as f32 / 65535.0
    }

    fn px(adjust: Adjust, rgb_px: [u16; 3]) -> [i32; 3] {
        let out = ToneStage(with(adjust)).apply(rgb(rgb_px.to_vec())).unwrap();
        [out.data[0] as i32, out.data[1] as i32, out.data[2] as i32]
    }

    #[test]
    fn saturation_scales_the_distance_from_the_luma() {
        let colour = [40000, 20000, 10000];
        assert_eq!(px(Adjust::default(), colour), colour.map(|v| v as i32));
        // -1 is gray: all channels equal the luma, which the slider keeps.
        let gray = px(
            Adjust {
                saturation: -1.0,
                ..Adjust::default()
            },
            colour,
        );
        assert!(gray[0] == gray[1] && gray[1] == gray[2], "{gray:?}");
        let luma = 0.2126 * 40000.0 + 0.7152 * 20000.0 + 0.0722 * 10000.0;
        assert!((gray[0] as f32 - luma).abs() <= 1.0);
        // Positive pulls the channels apart and keeps the luma.
        let vivid = px(
            Adjust {
                saturation: 0.5,
                ..Adjust::default()
            },
            colour,
        );
        assert!(vivid[0] - vivid[2] > colour[0] as i32 - colour[2] as i32);
        let l2 = 0.2126 * vivid[0] as f32 + 0.7152 * vivid[1] as f32 + 0.0722 * vivid[2] as f32;
        assert!((l2 - luma).abs() < 3.0, "{l2} vs {luma}");
        // Never past the range.
        let extreme = px(
            Adjust {
                saturation: 1.0,
                ..Adjust::default()
            },
            [65535, 0, 0],
        );
        assert!(extreme.iter().all(|v| (0..=65535).contains(v)));
    }

    #[test]
    fn saturation_leaves_gray_images_and_gray_pixels_alone() {
        let a = Adjust {
            saturation: 1.0,
            ..Adjust::default()
        };
        assert_eq!(ramp(a, &[0, 30000, 65535]), [0, 30000, 65535]);
        assert!(!a.is_neutral());
        assert_eq!(px(a, [30000, 30000, 30000]), [30000; 3]);
    }

    #[test]
    fn the_new_sliders_are_neutral_by_default_and_part_of_the_identity_check() {
        assert!(Adjust::default().is_neutral());
        for a in [
            Adjust {
                contrast: 0.1,
                ..Adjust::default()
            },
            Adjust {
                darks: -0.1,
                ..Adjust::default()
            },
            Adjust {
                whites: 0.1,
                ..Adjust::default()
            },
        ] {
            assert!(!a.is_neutral() && !with(a).is_identity());
        }
        let img = gray(vec![0, 1, 12345, 65535]);
        assert_eq!(
            ToneStage(with(Adjust::default()))
                .apply(img.clone())
                .unwrap(),
            img
        );
    }

    #[test]
    fn contrast_steepens_around_mid_grey_and_keeps_the_ends() {
        let up = Adjust {
            contrast: 0.8,
            ..Adjust::default()
        };
        assert!((at(up, 0.5) - 0.5).abs() < 1e-3, "mid-grey stays");
        assert!(
            at(up, 0.25) < 0.25 - 0.05,
            "darks get darker: {}",
            at(up, 0.25)
        );
        assert!(
            at(up, 0.75) > 0.75 + 0.05,
            "lights get lighter: {}",
            at(up, 0.75)
        );
        assert_eq!(ramp(up, &[0, 65535]), [0, 65535]);
        let down = Adjust {
            contrast: -0.8,
            ..Adjust::default()
        };
        assert!(at(down, 0.25) > 0.25 + 0.05 && at(down, 0.75) < 0.75 - 0.05);
    }

    #[test]
    fn darks_move_the_shadows_and_leave_black_and_the_highlights_alone() {
        let lift = Adjust {
            darks: 1.0,
            ..Adjust::default()
        };
        assert!(
            at(lift, 1.0 / 3.0) > 1.0 / 3.0 + 0.1,
            "{}",
            at(lift, 1.0 / 3.0)
        );
        assert_eq!(ramp(lift, &[0, 65535]), [0, 65535]);
        assert!(
            (at(lift, 0.95) - 0.95).abs() < 0.01,
            "highlights barely move"
        );
        let crush = Adjust {
            darks: -1.0,
            ..Adjust::default()
        };
        assert!(at(crush, 1.0 / 3.0) < 1.0 / 3.0 - 0.1);
    }

    #[test]
    fn whites_move_the_highlights_and_leave_white_and_the_shadows_alone() {
        let lift = Adjust {
            whites: 1.0,
            ..Adjust::default()
        };
        assert!(
            at(lift, 2.0 / 3.0) > 2.0 / 3.0 + 0.1,
            "{}",
            at(lift, 2.0 / 3.0)
        );
        assert_eq!(ramp(lift, &[0, 65535]), [0, 65535]);
        assert!((at(lift, 0.05) - 0.05).abs() < 0.01, "shadows barely move");
        let tame = Adjust {
            whites: -1.0,
            ..Adjust::default()
        };
        assert!(at(tame, 2.0 / 3.0) < 2.0 / 3.0 - 0.1);
    }

    #[test]
    fn no_slider_position_ever_inverts_the_tones() {
        let steps = [-1.0_f32, -0.5, 0.0, 0.5, 1.0];
        let input: Vec<u16> = (0..=1000)
            .map(|i| (i as f32 * 65.535).round() as u16)
            .collect();
        for contrast in steps {
            for darks in steps {
                for whites in steps {
                    let a = Adjust {
                        contrast,
                        darks,
                        whites,
                        ..Adjust::default()
                    };
                    let out = ramp(a, &input);
                    assert!(
                        out.windows(2).all(|w| w[0] <= w[1]),
                        "inverted tones for contrast {contrast}, darks {darks}, whites {whites}"
                    );
                    assert_eq!((out[0], out[1000]), (0, 65535), "{a:?}");
                }
            }
        }
    }

    #[test]
    fn tone_shaping_comes_after_levels_and_gamma() {
        // Black and white point first, then the contrast curve on the stretched range.
        let a = Adjust {
            black: 0.2,
            white: 0.8,
            contrast: 0.8,
            ..Adjust::default()
        };
        let max = 65535.0;
        let mid = at(a, 0.5);
        assert!(
            (mid - 0.5).abs() < 0.01,
            "the middle of the stretched range stays: {mid}"
        );
        assert!(
            at(a, 0.35) < 0.25,
            "0.35 is 25 % into 0.2..0.8, then darkened"
        );
        assert_eq!(
            ramp(a, &[(0.1 * max) as u16]),
            [0],
            "below the black point stays black"
        );
    }

    #[test]
    fn colour_images_get_the_same_curve_per_channel() {
        let a = Adjust {
            contrast: 0.7,
            darks: 0.4,
            whites: -0.3,
            ..Adjust::default()
        };
        let c = ToneStage(with(a))
            .apply(rgb(vec![20000, 20000, 20000]))
            .unwrap();
        let g = ToneStage(with(a)).apply(gray(vec![20000])).unwrap();
        assert_eq!(c.data, [g.data[0]; 3], "equal channels stay equal");
    }
}

#[cfg(test)]
mod manual {
    /// Prints percentiles of two image files: `A=/a.png B=/b.png cargo test -p latent-image-edit
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs two real image files"]
    fn compare_two_scans() {
        let (Ok(a), Ok(b)) = (std::env::var("A"), std::env::var("B")) else {
            return;
        };
        for (name, path) in [("A", a), ("B", b)] {
            let img = image::open(path).unwrap().into_luma16().into_raw();
            let mut v = img.clone();
            v.sort_unstable();
            let at = |p: f64| v[((v.len() - 1) as f64 * p) as usize] as f64 / 65535.0;
            let mean = img.iter().map(|&x| x as f64).sum::<f64>() / img.len() as f64 / 65535.0;
            println!(
                "{name}: mean {mean:.3}  p1 {:.3}  p25 {:.3}  p50 {:.3}  p75 {:.3}  p99 {:.3}",
                at(0.01),
                at(0.25),
                at(0.5),
                at(0.75),
                at(0.99)
            );
        }
    }
}
