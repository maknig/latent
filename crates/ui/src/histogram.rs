//! Luminance histogram of an image, drawn like the Qt original (white bars on black).
use eframe::egui;

pub type Histogram = [u32; 256];

/// Counts of luminance values (OpenCV RGB2GRAY weights).
pub fn luminance_histogram(im: &image::RgbaImage) -> Histogram {
    let mut h = [0u32; 256];
    for p in im.pixels() {
        let [r, g, b, _] = p.0;
        let y = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
        h[y.min(255) as usize] += 1;
    }
    h
}

/// Histogram plus the darkest and brightest spots (normalised 0..1 positions, x then y).
pub struct ImageStats {
    pub hist: Histogram,
    pub black: (f32, f32),
    pub white: (f32, f32),
}

pub fn image_stats(im: &image::RgbaImage) -> ImageStats {
    let (black, white) = black_white_points(im);
    ImageStats { hist: luminance_histogram(im), black, white }
}

/// Darkest and brightest spot of the blurred luminance image, like the Qt original
/// (Gaussian blur, then min/max location). Works on a copy of at most 512 px.
pub fn black_white_points(im: &image::RgbaImage) -> ((f32, f32), (f32, f32)) {
    const MAX: u32 = 512;
    const SIGMA: f32 = 1.3;
    let small;
    let im = if im.width().max(im.height()) > MAX {
        let k = MAX as f32 / im.width().max(im.height()) as f32;
        let (w, h) = (((im.width() as f32 * k).round() as u32).max(1), ((im.height() as f32 * k).round() as u32).max(1));
        small = image::imageops::resize(im, w, h, image::imageops::FilterType::Triangle);
        &small
    } else {
        im
    };
    let (w, h) = (im.width() as usize, im.height() as usize);
    if w == 0 || h == 0 {
        return ((0.5, 0.5), (0.5, 0.5));
    }
    let lum: Vec<f32> = im.pixels().map(|p| 0.299 * p.0[0] as f32 + 0.587 * p.0[1] as f32 + 0.114 * p.0[2] as f32).collect();
    let r = (3.0 * SIGMA).ceil() as isize;
    let mut kernel: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * SIGMA * SIGMA)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);
    let blur = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0; src.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (k, wgt) in kernel.iter().enumerate() {
                    let d = k as isize - r;
                    let (sx, sy) = if horizontal { ((x as isize + d).clamp(0, w as isize - 1) as usize, y) } else { (x, (y as isize + d).clamp(0, h as isize - 1) as usize) };
                    acc += wgt * src[sy * w + sx];
                }
                out[y * w + x] = acc;
            }
        }
        out
    };
    let blurred = blur(&blur(&lum, true), false);
    let (mut lo, mut hi) = (0, 0);
    for (i, &v) in blurred.iter().enumerate() {
        if v < blurred[lo] {
            lo = i;
        }
        if v > blurred[hi] {
            hi = i;
        }
    }
    let pos = |i: usize| (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32);
    (pos(lo), pos(hi))
}

/// Draw `h` into a `size` rect; bar heights are relative to the tallest bin.
pub fn paint(ui: &mut egui::Ui, h: &Histogram, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    let max = h.iter().copied().max().unwrap_or(0).max(1) as f32;
    let step = rect.width() / h.len() as f32;
    for (i, &n) in h.iter().enumerate() {
        if n == 0 {
            continue;
        }
        let x = rect.left() + (i as f32 + 0.5) * step;
        let top = rect.bottom() - rect.height() * n as f32 / max;
        painter.line_segment([egui::pos2(x, top), egui::pos2(x, rect.bottom())], egui::Stroke::new(step.max(1.0), egui::Color32::WHITE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_and_split_images() {
        let black = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        let h = luminance_histogram(&black);
        assert_eq!((h[0], h.iter().sum::<u32>()), (16, 16));
        let mut split = image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255]));
        for x in 0..2 {
            for y in 0..4 {
                split.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let h = luminance_histogram(&split);
        assert_eq!((h[0], h[255]), (8, 8));
    }

    #[test]
    fn black_and_white_points_follow_blobs() {
        let mut im = image::RgbaImage::from_pixel(200, 100, image::Rgba([128, 128, 128, 255]));
        for y in 10..20 {
            for x in 150..160 {
                im.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
            for x in 30..40 {
                im.put_pixel(x, y + 60, image::Rgba([0, 0, 0, 255]));
            }
        }
        im.put_pixel(100, 50, image::Rgba([255, 255, 255, 255])); // lone outlier is blurred away
        let (black, white) = black_white_points(&im);
        assert!((black.0 - 0.175).abs() < 0.03 && (black.1 - 0.75).abs() < 0.05, "{black:?}");
        assert!((white.0 - 0.775).abs() < 0.03 && (white.1 - 0.15).abs() < 0.05, "{white:?}");
    }
}
