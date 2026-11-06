//! Unsharp mask on brightness only, so colours do not fringe or shift.
//!
//! The luma of the image is blurred with a Gaussian; the difference between luma and blur,
//! times the amount, is added to every channel. Spatial, hence not part of the per-sample
//! tone lookup table.

use crate::Stage;
use crate::{RawImage, Result};

/// Smallest radius (px) that still blurs; below it sharpening does nothing visible.
const MIN_RADIUS: f32 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sharpen {
    /// Strength, `0.0..=3.0` (0 = off, 1 = the difference to the blur is added once).
    pub amount: f32,
    /// Gaussian radius (sigma) in pixels of the image being sharpened, `0.3..=5.0`.
    pub radius: f32,
}

impl Default for Sharpen {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius: 1.0,
        }
    }
}

impl Sharpen {
    pub fn is_neutral(&self) -> bool {
        self.amount <= 0.0
    }

    /// The same sharpening for an image `factor` times the size of this one (a downscaled
    /// preview of the file): the radius shrinks with the image.
    pub fn scaled(&self, factor: f32) -> Self {
        Self {
            radius: self.radius * factor,
            ..*self
        }
    }
}

/// Normalized Gaussian weights for `sigma`, truncated at three sigma.
fn kernel(sigma: f32) -> Vec<f32> {
    let reach = (3.0 * sigma).ceil().max(1.0) as i32;
    let mut k: Vec<f32> = (-reach..=reach)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|w| *w /= sum);
    k
}

/// Separable Gaussian blur of a `w` x `h` plane, edges clamped.
pub(crate) fn blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let k = kernel(sigma);
    let reach = k.len() / 2;
    let mut tmp = vec![0f32; plane.len()];
    // Rows: pad the row once with its clamped edges so the taps need no bounds games.
    let mut padded = vec![0f32; w + 2 * reach];
    for (row, out) in plane.chunks_exact(w).zip(tmp.chunks_exact_mut(w)) {
        padded[..reach].fill(row[0]);
        padded[reach..reach + w].copy_from_slice(row);
        padded[reach + w..].fill(row[w - 1]);
        for (x, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (weight, v) in k.iter().zip(&padded[x..x + k.len()]) {
                acc += weight * v;
            }
            *o = acc;
        }
    }
    // Columns: whole rows are added up, which walks memory in order and vectorises.
    let mut out = vec![0f32; plane.len()];
    for (y, out_row) in out.chunks_exact_mut(w).enumerate() {
        for (i, weight) in k.iter().enumerate() {
            let ys = (y as isize + i as isize - reach as isize).clamp(0, h as isize - 1) as usize;
            for (o, v) in out_row.iter_mut().zip(&tmp[ys * w..(ys + 1) * w]) {
                *o += weight * v;
            }
        }
    }
    out
}

pub(crate) fn luma(img: &RawImage) -> Vec<f32> {
    if img.channels == 3 {
        img.data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
            .collect()
    } else {
        img.data.iter().map(|&v| v as f32).collect()
    }
}

/// Sharpens `img` in place.
pub fn sharpen(img: &mut RawImage, s: Sharpen) {
    if s.is_neutral() || s.radius < MIN_RADIUS || img.width == 0 || img.height == 0 {
        return;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let y = luma(img);
    let blurred = blur(&y, w, h, s.radius);
    let n = img.channels as usize;
    for (i, px) in img.data.chunks_exact_mut(n).enumerate() {
        let delta = s.amount * (y[i] - blurred[i]);
        for v in px {
            *v = (*v as f32 + delta).round().clamp(0.0, u16::MAX as f32) as u16;
        }
    }
}

/// [`sharpen`] as a pipeline stage.
pub struct SharpenStage(pub Sharpen);

impl Stage for SharpenStage {
    fn apply(&self, mut img: RawImage) -> Result<RawImage> {
        sharpen(&mut img, self.0);
        Ok(img)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The straightforward blur with a clamp per tap, for comparison.
    fn naive_blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
        let k = kernel(sigma);
        let reach = (k.len() / 2) as isize;
        let at = |x: isize, y: isize| {
            plane[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize]
        };
        let mut tmp = vec![0f32; plane.len()];
        for y in 0..h {
            for x in 0..w {
                tmp[y * w + x] = k
                    .iter()
                    .enumerate()
                    .map(|(i, wt)| wt * at(x as isize + i as isize - reach, y as isize))
                    .sum();
            }
        }
        let at2 = |x: isize, y: isize| tmp[y.clamp(0, h as isize - 1) as usize * w + x as usize];
        let mut out = vec![0f32; plane.len()];
        for y in 0..h {
            for x in 0..w {
                out[y * w + x] = k
                    .iter()
                    .enumerate()
                    .map(|(i, wt)| wt * at2(x as isize, y as isize + i as isize - reach))
                    .sum();
            }
        }
        out
    }

    #[test]
    fn the_fast_blur_equals_the_naive_one() {
        for (w, h, sigma) in [
            (23usize, 17usize, 1.5f32),
            (5, 40, 4.0),
            (1, 9, 2.0),
            (31, 3, 0.7),
        ] {
            let plane: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 1000) as f32).collect();
            let (fast, naive) = (blur(&plane, w, h, sigma), naive_blur(&plane, w, h, sigma));
            for (a, b) in fast.iter().zip(&naive) {
                assert!((a - b).abs() < 0.05, "{w}x{h} sigma {sigma}: {a} vs {b}");
            }
        }
    }

    /// A gray image `w` wide whose left half is `lo` and right half `hi`.
    fn step(w: u32, lo: u16, hi: u16) -> RawImage {
        let mut data = vec![];
        for _y in 0..5 {
            for x in 0..w {
                data.push(if x < w / 2 { lo } else { hi });
            }
        }
        RawImage::new(w, 5, 1, data).unwrap()
    }

    fn row(img: &RawImage) -> Vec<i32> {
        img.data[..img.width as usize]
            .iter()
            .map(|&v| v as i32)
            .collect()
    }

    fn with(amount: f32, radius: f32) -> Sharpen {
        Sharpen { amount, radius }
    }

    #[test]
    fn the_kernel_is_normalized_and_symmetric() {
        for sigma in [0.3, 1.0, 2.5] {
            let k = kernel(sigma);
            assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            assert!(k
                .iter()
                .zip(k.iter().rev())
                .all(|(a, b)| (a - b).abs() < 1e-6));
            assert_eq!(k.len() % 2, 1);
        }
    }

    #[test]
    fn amount_zero_and_flat_images_stay_as_they_are() {
        let img = step(40, 10000, 50000);
        let mut same = img.clone();
        sharpen(&mut same, with(0.0, 2.0));
        assert_eq!(same, img);

        let mut flat = RawImage::new(30, 4, 1, vec![30000; 120]).unwrap();
        sharpen(&mut flat, with(2.0, 2.0));
        assert!(flat.data.iter().all(|&v| v == 30000), "no edges, no change");
    }

    #[test]
    fn an_edge_gets_overshoot_on_both_sides() {
        let mut img = step(60, 20000, 40000);
        sharpen(&mut img, with(1.5, 2.0));
        let r = row(&img);
        let at_edge = 30;
        assert!(
            r[at_edge - 1] < 20000 - 500,
            "dark side dips: {}",
            r[at_edge - 1]
        );
        assert!(
            r[at_edge] > 40000 + 500,
            "bright side peaks: {}",
            r[at_edge]
        );
        assert_eq!(
            (r[5], r[55]),
            (20000, 40000),
            "far from the edge nothing changes"
        );
    }

    #[test]
    fn the_result_stays_inside_16_bits() {
        let mut img = step(60, 0, 65535);
        sharpen(&mut img, with(3.0, 1.0));
        assert_eq!(*img.data.iter().min().unwrap(), 0);
        assert_eq!(*img.data.iter().max().unwrap(), 65535);
    }

    #[test]
    fn a_larger_radius_makes_a_wider_halo() {
        let reach = |radius: f32| {
            let mut img = step(80, 20000, 40000);
            sharpen(&mut img, with(1.0, radius));
            let r = row(&img);
            // How many pixels left of the edge are still changed.
            (0..40).rev().take_while(|&x| r[x] != 20000).count()
        };
        assert!(reach(3.0) > reach(1.0), "{} vs {}", reach(3.0), reach(1.0));
    }

    #[test]
    fn colour_channels_get_the_same_change_and_neutral_grey_stays_neutral() {
        let mut data = vec![];
        for _y in 0..5 {
            for x in 0..40 {
                let v = if x < 20 { 15000 } else { 45000 };
                data.extend([v, v, v]);
            }
        }
        let mut img = RawImage::new(40, 5, 3, data).unwrap();
        sharpen(&mut img, with(1.0, 1.5));
        for px in img.data.as_chunks::<3>().0 {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2], "no colour shift from sharpening");
        }
        assert!(img.data[3 * 20] > 45000, "the edge is sharpened");
    }

    #[test]
    fn a_coloured_edge_shifts_every_channel_by_the_same_amount() {
        // Red and blue differ, the brightness edge is shared: deltas are equal per channel.
        let mut data = vec![];
        for _y in 0..5 {
            for x in 0..40 {
                if x < 20 {
                    data.extend([30000u16, 20000, 10000]);
                } else {
                    data.extend([50000u16, 40000, 30000]);
                }
            }
        }
        let before = data.clone();
        let mut img = RawImage::new(40, 5, 3, data).unwrap();
        sharpen(&mut img, with(1.0, 1.5));
        let i = 3 * 20;
        let deltas: Vec<i32> = (0..3)
            .map(|c| img.data[i + c] as i32 - before[i + c] as i32)
            .collect();
        assert!(
            deltas[0] != 0 && deltas.iter().all(|d| (d - deltas[0]).abs() <= 1),
            "{deltas:?}"
        );
    }

    #[test]
    fn scaling_adapts_the_radius_to_a_downscaled_image() {
        let s = with(1.0, 3.0).scaled(0.25);
        assert_eq!((s.amount, s.radius), (1.0, 0.75));
        // Below the minimum radius nothing is done.
        let mut img = step(40, 10000, 50000);
        let before = img.clone();
        sharpen(&mut img, with(2.0, 3.0).scaled(0.05));
        assert_eq!(img, before);
    }

    #[test]
    fn size_and_channel_count_are_kept_and_the_stage_works() {
        let img = step(33, 1000, 60000);
        let out = SharpenStage(with(1.0, 1.0)).apply(img.clone()).unwrap();
        assert_eq!((out.width, out.height, out.channels), (33, 5, 1));
        assert_ne!(out, img);
    }
}
