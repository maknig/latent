//! Red, green and blue histogram of the edited preview.

use egui::{Color32, ColorImage, Pos2, Rect, Stroke, Ui, Vec2};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    pub r: [u32; 256],
    pub g: [u32; 256],
    pub b: [u32; 256],
    /// All pixels have equal channels: drawn as one grey curve.
    pub gray: bool,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            r: [0; 256],
            g: [0; 256],
            b: [0; 256],
            gray: true,
        }
    }
}

impl Histogram {
    /// Counts the 8-bit channels of a display image.
    pub fn of(img: &ColorImage) -> Self {
        let mut h = Self::default();
        for p in &img.pixels {
            h.r[p.r() as usize] += 1;
            h.g[p.g() as usize] += 1;
            h.b[p.b() as usize] += 1;
            h.gray &= p.r() == p.g() && p.g() == p.b();
        }
        h
    }

    /// Counts the channels of an RGBA image (alpha is ignored).
    pub fn of_rgba(img: &image::RgbaImage) -> Self {
        let mut h = Self::default();
        for p in img.pixels() {
            let [r, g, b, _] = p.0;
            h.r[r as usize] += 1;
            h.g[g as usize] += 1;
            h.b[b as usize] += 1;
            h.gray &= r == g && g == b;
        }
        h
    }

    /// The height the curves are scaled to: the tallest bin *except* the two end bins, so a
    /// spike of clipped pixels at 0 or 255 does not flatten the rest. At least 1.
    pub fn scale(&self) -> u32 {
        [&self.r, &self.g, &self.b]
            .iter()
            .flat_map(|c| c[1..255].iter().copied())
            .max()
            .unwrap_or(1)
            .max(1)
    }

    /// Pixels counted (the same for every channel).
    pub fn total(&self) -> u64 {
        self.r.iter().map(|&c| c as u64).sum()
    }

    /// Bar heights `0.0..=1.0` of a channel for the given scale (end-bin spikes are cut off).
    pub fn heights(channel: &[u32; 256], scale: u32) -> Vec<f32> {
        channel
            .iter()
            .map(|&c| (c as f32 / scale as f32).min(1.0))
            .collect()
    }
}

/// Draws the histogram in a box of `size`.
pub fn paint(ui: &mut Ui, h: &Histogram, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_gray(18));
    let grid = Stroke::new(1.0_f32, Color32::from_white_alpha(25));
    for i in 1..4 {
        let x = rect.left() + rect.width() * i as f32 / 4.0;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            grid,
        );
    }
    let scale = h.scale();
    let bar = rect.width() / 256.0;
    let channels: Vec<(&[u32; 256], Color32)> = if h.gray {
        vec![(&h.g, Color32::from_rgba_unmultiplied(210, 210, 210, 150))]
    } else {
        vec![
            (&h.r, Color32::from_rgba_unmultiplied(235, 60, 60, 110)),
            (&h.g, Color32::from_rgba_unmultiplied(60, 210, 80, 110)),
            (&h.b, Color32::from_rgba_unmultiplied(70, 110, 245, 110)),
        ]
    };
    for (channel, color) in channels {
        for (i, height) in Histogram::heights(channel, scale).into_iter().enumerate() {
            if height <= 0.0 {
                continue;
            }
            let x = rect.left() + i as f32 * bar;
            let bar_rect = Rect::from_min_max(
                Pos2::new(x, rect.bottom() - height * (rect.height() - 2.0)),
                Pos2::new(x + bar.max(1.0), rect.bottom()),
            );
            painter.rect_filled(bar_rect, 0.0, color);
        }
    }
}

/// Where to mark the darkest, brightest and most neutral spot of a picture (normalised 0..1
/// positions, x then y).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Markers {
    pub black: (f32, f32),
    pub white: (f32, f32),
    /// `None` for gray pictures, which have no colour to be neutral.
    pub gray: Option<(f32, f32)>,
}

/// Lowest and highest luma (of 255) where a pixel can be the neutral spot: pure black and
/// blown-out pixels have no cast to measure.
const GRAY_LUMA: std::ops::RangeInclusive<f32> = 38.0..=230.0;

/// Darkest and brightest spot of the blurred luminance image (Gaussian blur, then min/max
/// location) and the spot with the least colour among the mid-tones. Works on a copy of at most
/// 512 px.
pub fn marker_points(im: &image::RgbaImage) -> Markers {
    const MAX: u32 = 512;
    const SIGMA: f32 = 1.3;
    let small;
    let im = if im.width().max(im.height()) > MAX {
        let k = MAX as f32 / im.width().max(im.height()) as f32;
        let (w, h) = (
            ((im.width() as f32 * k).round() as u32).max(1),
            ((im.height() as f32 * k).round() as u32).max(1),
        );
        small = image::imageops::resize(im, w, h, image::imageops::FilterType::Triangle);
        &small
    } else {
        im
    };
    let (w, h) = (im.width() as usize, im.height() as usize);
    if w == 0 || h == 0 {
        let mid = (0.5, 0.5);
        return Markers {
            black: mid,
            white: mid,
            gray: None,
        };
    }
    let r = (3.0 * SIGMA).ceil() as isize;
    let mut kernel: Vec<f32> = (-r..=r)
        .map(|i| (-((i * i) as f32) / (2.0 * SIGMA * SIGMA)).exp())
        .collect();
    let sum: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);
    let blur = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0; src.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (k, wgt) in kernel.iter().enumerate() {
                    let d = k as isize - r;
                    let (sx, sy) = if horizontal {
                        ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                    } else {
                        (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                    };
                    acc += wgt * src[sy * w + sx];
                }
                out[y * w + x] = acc;
            }
        }
        out
    };
    let plane = |c: usize| -> Vec<f32> {
        let src: Vec<f32> = im.pixels().map(|p| p.0[c] as f32).collect();
        blur(&blur(&src, true), false)
    };
    let (pr, pg, pb) = (plane(0), plane(1), plane(2));
    let luma = |i: usize| 0.299 * pr[i] + 0.587 * pg[i] + 0.114 * pb[i];
    let colourful = im.pixels().any(|p| p.0[0] != p.0[1] || p.0[1] != p.0[2]);
    let (mut lo, mut hi) = (0, 0);
    let mut gray: Option<(usize, f32)> = None;
    for i in 0..w * h {
        let l = luma(i);
        if l < luma(lo) {
            lo = i;
        }
        if l > luma(hi) {
            hi = i;
        }
        if colourful && GRAY_LUMA.contains(&l) {
            let (max, min) = (pr[i].max(pg[i]).max(pb[i]), pr[i].min(pg[i]).min(pb[i]));
            let chroma = (max - min) / ((pr[i] + pg[i] + pb[i]) / 3.0).max(1.0);
            if gray.is_none_or(|(_, best)| chroma < best) {
                gray = Some((i, chroma));
            }
        }
    }
    let pos = |i: usize| {
        (
            ((i % w) as f32 + 0.5) / w as f32,
            ((i / w) as f32 + 0.5) / h as f32,
        )
    };
    Markers {
        black: pos(lo),
        white: pos(hi),
        gray: gray.map(|(i, _)| pos(i)),
    }
}

#[cfg(test)]
mod tests {
    fn solid(w: u32, h: u32, c: [u8; 3]) -> image::RgbaImage {
        image::RgbaImage::from_pixel(w, h, image::Rgba([c[0], c[1], c[2], 255]))
    }

    #[test]
    fn black_and_white_points_follow_blobs() {
        let mut im = solid(200, 100, [128, 128, 128]);
        for y in 10..20 {
            for x in 150..160 {
                im.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
            for x in 30..40 {
                im.put_pixel(x, y + 60, image::Rgba([0, 0, 0, 255]));
            }
        }
        im.put_pixel(100, 50, image::Rgba([255, 255, 255, 255])); // lone outlier is blurred away
        let m = marker_points(&im);
        let (black, white) = (m.black, m.white);
        assert!(
            (black.0 - 0.175).abs() < 0.03 && (black.1 - 0.75).abs() < 0.05,
            "{black:?}"
        );
        assert!(
            (white.0 - 0.775).abs() < 0.03 && (white.1 - 0.15).abs() < 0.05,
            "{white:?}"
        );
        assert_eq!(m.gray, None, "a gray picture has no neutral spot to find");
    }

    #[test]
    fn the_gray_marker_finds_the_neutral_patch_in_a_coloured_picture() {
        let mut im = solid(200, 100, [200, 120, 80]);
        for y in 60..80 {
            for x in 20..40 {
                im.put_pixel(x, y, image::Rgba([130, 130, 130, 255]));
            }
        }
        // Neutral but black / white pixels do not count.
        for x in 150..170 {
            im.put_pixel(x, 20, image::Rgba([0, 0, 0, 255]));
            im.put_pixel(x, 30, image::Rgba([255, 255, 255, 255]));
        }
        let (x, y) = marker_points(&im).gray.unwrap();
        assert!((x - 0.15).abs() < 0.05 && (y - 0.7).abs() < 0.08, "{x} {y}");
    }

    #[test]
    fn the_gray_marker_picks_the_least_coloured_spot_of_a_colourful_picture() {
        let mut im = solid(100, 100, [220, 100, 60]);
        for y in 0..100 {
            for x in 50..100 {
                im.put_pixel(x, y, image::Rgba([150, 130, 120, 255]));
            }
        }
        let (x, _) = marker_points(&im).gray.unwrap();
        assert!(x > 0.5, "{x}");
    }

    #[test]
    fn rgba_counts_match_the_colour_image_counts() {
        let rgba = image::RgbaImage::from_fn(4, 2, |x, y| {
            image::Rgba([(x * 60) as u8, (y * 100) as u8, 7, 255])
        });
        let color = ColorImage::from_rgba_unmultiplied([4, 2], rgba.as_raw());
        assert_eq!(Histogram::of_rgba(&rgba), Histogram::of(&color));
        assert!(!Histogram::of_rgba(&rgba).gray);
    }

    use super::*;

    fn image(pixels: &[(u8, u8, u8)]) -> ColorImage {
        let rgb: Vec<u8> = pixels.iter().flat_map(|&(r, g, b)| [r, g, b]).collect();
        ColorImage::from_rgb([pixels.len(), 1], &rgb)
    }

    #[test]
    fn counts_every_pixel_in_every_channel() {
        let h = Histogram::of(&image(&[(0, 10, 255), (0, 20, 255), (7, 20, 1)]));
        assert_eq!(h.total(), 3);
        assert_eq!(h.g.iter().map(|&c| c as u64).sum::<u64>(), 3);
        assert_eq!(h.b.iter().map(|&c| c as u64).sum::<u64>(), 3);
        assert_eq!((h.r[0], h.r[7]), (2, 1));
        assert_eq!((h.g[10], h.g[20]), (1, 2));
        assert_eq!((h.b[255], h.b[1]), (2, 1));
    }

    #[test]
    fn grey_images_are_detected() {
        assert!(Histogram::of(&image(&[(5, 5, 5), (200, 200, 200)])).gray);
        assert!(!Histogram::of(&image(&[(5, 5, 5), (200, 201, 200)])).gray);
        assert!(
            Histogram::of(&ColorImage::from_rgb([0, 0], &[])).gray,
            "empty counts as grey"
        );
    }

    #[test]
    fn the_scale_ignores_the_end_bins_so_a_spike_does_not_flatten_the_rest() {
        let mut pixels = vec![(0, 0, 0); 1000];
        pixels.extend(vec![(100, 100, 100); 10]);
        pixels.extend(vec![(255, 255, 255); 500]);
        let h = Histogram::of(&image(&pixels));
        assert_eq!(h.scale(), 10, "set by the biggest of the inner bins");
        let heights = Histogram::heights(&h.r, h.scale());
        assert_eq!(
            (heights[0], heights[255]),
            (1.0, 1.0),
            "the spikes are cut off at full height"
        );
        assert_eq!(heights[100], 1.0);
        assert_eq!(heights[50], 0.0);
    }

    #[test]
    fn an_empty_histogram_has_a_usable_scale() {
        assert_eq!(Histogram::default().scale(), 1);
        assert!(Histogram::heights(&[0; 256], 1).iter().all(|&h| h == 0.0));
    }

    #[test]
    fn colour_images_keep_their_own_channel_curves() {
        let h = Histogram::of(&image(&[(255, 0, 0), (255, 0, 0)]));
        assert!(!h.gray);
        assert_eq!((h.r[255], h.g[0], h.b[0]), (2, 2, 2));
    }
}
