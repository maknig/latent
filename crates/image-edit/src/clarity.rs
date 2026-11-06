//! Clarity: local contrast in the midtones.
//!
//! The luma is compared with a very smooth version of itself (a Gaussian whose radius is a
//! fixed fraction of the image, so a downscaled preview looks like the full file); the
//! difference, weighted towards the midtones, is added to every channel. Positive values
//! crisp up the mid-sized structure, negative values soften it. Black and white stay put.

use crate::sharpen::{blur, luma};
use crate::{RawImage, Result, Stage};

/// Blur radius (sigma) as a fraction of the longer side.
const RADIUS: f32 = 0.02;
/// The blur runs on a plane shrunk until its sigma is about this many pixels.
const WORK_SIGMA: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clarity {
    /// `-1.0..=1.0`: negative softens, 0 = off, positive adds local contrast.
    pub amount: f32,
}

impl Default for Clarity {
    fn default() -> Self {
        Self { amount: 0.0 }
    }
}

impl Clarity {
    pub fn is_neutral(&self) -> bool {
        self.amount == 0.0
    }
}

/// The luma plane shrunk by block averaging, `factor` pixels per cell, as (plane, w, h).
fn shrink(plane: &[f32], w: usize, h: usize, factor: usize) -> (Vec<f32>, usize, usize) {
    let (sw, sh) = (w.div_ceil(factor), h.div_ceil(factor));
    let mut sum = vec![0f32; sw * sh];
    let mut count = vec![0f32; sw * sh];
    for y in 0..h {
        for x in 0..w {
            let i = (y / factor) * sw + x / factor;
            sum[i] += plane[y * w + x];
            count[i] += 1.0;
        }
    }
    for (s, c) in sum.iter_mut().zip(&count) {
        *s /= c;
    }
    (sum, sw, sh)
}

/// Bilinear value of the small plane at the centre of full-size pixel (`x`, `y`).
fn sample(small: &[f32], sw: usize, sh: usize, factor: usize, x: usize, y: usize) -> f32 {
    let f = factor as f32;
    let fx = ((x as f32 + 0.5) / f - 0.5).clamp(0.0, sw as f32 - 1.0);
    let fy = ((y as f32 + 0.5) / f - 0.5).clamp(0.0, sh as f32 - 1.0);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let top = small[y0 * sw + x0] * (1.0 - tx) + small[y0 * sw + x1] * tx;
    let bottom = small[y1 * sw + x0] * (1.0 - tx) + small[y1 * sw + x1] * tx;
    top * (1.0 - ty) + bottom * ty
}

/// Applies clarity to `img` in place.
pub fn clarity(img: &mut RawImage, c: Clarity) {
    if c.is_neutral() || img.width == 0 || img.height == 0 {
        return;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let sigma = RADIUS * w.max(h) as f32;
    let factor = (sigma / WORK_SIGMA).floor().max(1.0) as usize;
    let y = luma(img);
    let (small, sw, sh) = shrink(&y, w, h, factor);
    let base = blur(&small, sw, sh, (sigma / factor as f32).max(0.5));
    let n = img.channels as usize;
    let full = u16::MAX as f32;
    for (i, px) in img.data.chunks_exact_mut(n).enumerate() {
        let (x, row) = (i % w, i / w);
        let l = (y[i] / full).clamp(0.0, 1.0);
        // 1 at mid-grey, 0 at black and white: no clipping, no halos in the extremes.
        let weight = 4.0 * l * (1.0 - l);
        let delta = c.amount * weight * (y[i] - sample(&base, sw, sh, factor, x, row));
        for v in px {
            *v = (*v as f32 + delta).round().clamp(0.0, full) as u16;
        }
    }
}

/// [`clarity`] as a pipeline stage.
pub struct ClarityStage(pub Clarity);

impl Stage for ClarityStage {
    fn apply(&self, mut img: RawImage) -> Result<RawImage> {
        clarity(&mut img, self.0);
        Ok(img)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gray stripes, `period` pixels long, between `lo` and `hi`, `w` x 6.
    fn stripes(w: u32, period: u32, lo: u16, hi: u16) -> RawImage {
        let data = (0..6)
            .flat_map(|_| {
                (0..w).map(move |x| {
                    if (x / (period / 2)).is_multiple_of(2) {
                        lo
                    } else {
                        hi
                    }
                })
            })
            .collect();
        RawImage::new(w, 6, 1, data).unwrap()
    }

    /// Spread of the middle of the first row (the edges of the image are clamped).
    fn range(img: &RawImage) -> i32 {
        let mid = &img.data[img.width as usize / 3..img.width as usize * 2 / 3];
        *mid.iter().max().unwrap() as i32 - *mid.iter().min().unwrap() as i32
    }

    fn with(img: &RawImage, amount: f32) -> RawImage {
        let mut out = img.clone();
        clarity(&mut out, Clarity { amount });
        out
    }

    #[test]
    fn zero_and_flat_images_stay_as_they_are() {
        let img = stripes(600, 60, 24000, 28000);
        assert_eq!(with(&img, 0.0), img);
        let flat = RawImage::new(100, 6, 1, vec![30000; 600]).unwrap();
        assert_eq!(with(&flat, 1.0), flat);
    }

    #[test]
    fn positive_adds_and_negative_removes_local_contrast() {
        let img = stripes(600, 60, 24000, 28000);
        assert!(range(&with(&img, 1.0)) > range(&img) + 1000);
        assert!(range(&with(&img, -1.0)) < range(&img) - 300);
    }

    #[test]
    fn black_and_white_are_not_moved() {
        let mut data = vec![0u16; 300];
        data.extend(vec![65535u16; 300]);
        data.extend((0..300).map(|i| 20000 + (i % 2) * 8000));
        let img = RawImage::new(300, 3, 1, data).unwrap();
        for amount in [1.0, -1.0] {
            let out = with(&img, amount);
            assert!(out.data[..300].iter().all(|&v| v == 0), "black");
            assert!(out.data[300..600].iter().all(|&v| v == 65535), "white");
        }
    }

    #[test]
    fn colour_channels_get_the_same_shift() {
        let gray = stripes(600, 60, 24000, 28000);
        let rgb = RawImage::new(
            600,
            6,
            3,
            gray.data.iter().flat_map(|&v| [v, v, v]).collect(),
        )
        .unwrap();
        let (g, c) = (with(&gray, 1.0), with(&rgb, 1.0));
        for (i, px) in c.data.as_chunks::<3>().0.iter().enumerate() {
            assert!(px.iter().all(|&v| v == g.data[i]));
        }
    }

    #[test]
    fn a_half_size_copy_gets_the_same_look() {
        // The radius follows the image size, so a preview matches the file.
        let big = stripes(1200, 120, 24000, 28000);
        let small = stripes(600, 60, 24000, 28000);
        let (b, s) = (with(&big, 1.0), with(&small, 1.0));
        for x in (0..600).step_by(7) {
            let want = s.data[x] as i32;
            let got = b.data[x * 2] as i32;
            assert!((want - got).abs() < 400, "x {x}: {got} vs {want}");
        }
    }
}
