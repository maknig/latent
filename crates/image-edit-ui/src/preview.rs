//! Display conversion of scans: 16-bit samples to a downscaled 8-bit egui image.

use egui::ColorImage;
use latent_image_edit::tone::{to_gray, Tone, ToneOptions};
use latent_image_edit::Crop;
use latent_image_edit::RawImage;

/// How the preview is shown: the same toning and colour mode the saved scan will get.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Look {
    pub tone: ToneOptions,
    pub gray: bool,
    /// Clockwise rotation of the picture in degrees (0, 90, 180, 270).
    pub rotate: u16,
}

/// Downscale so the longer side is at most `max_side`, keeping 16 bits: every output sample is
/// the average of the `step x step` block it covers (picking single pixels would alias fine
/// edges).
pub fn downscale(img: &RawImage, max_side: u32) -> RawImage {
    let step = img.width.max(img.height).div_ceil(max_side.max(1)).max(1) as usize;
    if step == 1 {
        return img.clone();
    }
    let (w, h, c) = (
        img.width as usize,
        img.height as usize,
        img.channels as usize,
    );
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let mut data = Vec::with_capacity(ow * oh * c);
    let mut sum = vec![0u32; c];
    for oy in 0..oh {
        let (y0, y1) = (oy * step, ((oy + 1) * step).min(h));
        for ox in 0..ow {
            let (x0, x1) = (ox * step, ((ox + 1) * step).min(w));
            sum.fill(0);
            for y in y0..y1 {
                for px in img.data[(y * w + x0) * c..(y * w + x1) * c].chunks_exact(c) {
                    for (s, &v) in sum.iter_mut().zip(px) {
                        *s += v as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            data.extend(sum.iter().map(|&s| ((s + n / 2) / n) as u16));
        }
    }
    RawImage {
        width: ow as u32,
        height: oh as u32,
        channels: img.channels,
        data,
    }
}

/// Turns an image clockwise by `deg` (0, 90, 180, 270); 90 and 270 swap width and height.
pub fn rotate_image(img: &ColorImage, deg: u16) -> ColorImage {
    let [w, h] = img.size;
    let mut out = img.clone();
    match deg % 360 {
        90 => {
            out.size = [h, w];
            for y in 0..h {
                for x in 0..w {
                    out.pixels[x * h + (h - 1 - y)] = img.pixels[y * w + x];
                }
            }
        }
        180 => out.pixels.reverse(),
        270 => {
            out.size = [h, w];
            for y in 0..h {
                for x in 0..w {
                    out.pixels[(w - 1 - x) * h + y] = img.pixels[y * w + x];
                }
            }
        }
        _ => {}
    }
    out
}

/// The pixels of `img` inside `frame` (normalized `[l, t, r, b]`), at least one pixel.
pub fn region(img: &RawImage, frame: [f32; 4]) -> Option<RawImage> {
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let (w, h) = (img.width as f32, img.height as f32);
    let x = (frame[0] * w).floor().clamp(0.0, w - 1.0) as u32;
    let y = (frame[1] * h).floor().clamp(0.0, h - 1.0) as u32;
    let cw = (((frame[2] - frame[0]) * w).ceil() as u32).clamp(1, img.width - x);
    let ch = (((frame[3] - frame[1]) * h).ceil() as u32).clamp(1, img.height - y);
    Crop {
        x,
        y,
        width: cw,
        height: ch,
    }
    .of(img)
    .ok()
}

/// Renders the overview for display. The tone is estimated from `reference` (a frame, as
/// normalized `[l, t, r, b]`) and applied to the whole image, so the preview shows what the
/// scan of that frame will look like.
pub fn render(raw: &RawImage, look: Look, reference: [f32; 4]) -> ColorImage {
    let base = if look.gray { to_gray(raw) } else { raw.clone() };
    if look.tone.is_identity() {
        return rotate_image(&to_color_image(&base, u32::MAX), look.rotate);
    }
    let tone = match region(&base, reference) {
        Some(r) => Tone::estimate(&r, look.tone),
        None => Tone::estimate(&base, look.tone),
    };
    let mut shown = base;
    tone.apply(&mut shown);
    rotate_image(&to_color_image(&shown, u32::MAX), look.rotate)
}

/// Nearest-neighbour downscale so the longer side is at most `max_side`; 16 -> 8 bit.
pub fn to_color_image(img: &RawImage, max_side: u32) -> ColorImage {
    let step = img.width.max(img.height).div_ceil(max_side.max(1)).max(1) as usize;
    let (w, h, c) = (
        img.width as usize,
        img.height as usize,
        img.channels as usize,
    );
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let mut rgb = Vec::with_capacity(ow * oh * 3);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let px = &img.data[(y * w + x) * c..][..c];
            let s = |i: usize| (px[i.min(c - 1)] >> 8) as u8;
            rgb.extend_from_slice(&[s(0), s(1), s(2)]);
        }
    }
    ColorImage::from_rgb([ow, oh], &rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    #[test]
    fn gray_is_widened_to_rgb_and_depth_reduced() {
        let img = RawImage::new(2, 1, 1, vec![0xFFFF, 0x8000]).unwrap();
        let c = to_color_image(&img, 100);
        assert_eq!(c.size, [2, 1]);
        assert_eq!(
            c.pixels,
            vec![
                Color32::from_rgb(255, 255, 255),
                Color32::from_rgb(128, 128, 128)
            ]
        );
    }

    fn three_by_one(px: [[u16; 3]; 3]) -> RawImage {
        RawImage::new(3, 1, 3, px.concat()).unwrap()
    }

    /// A 3x2 image whose pixels are numbered 0..6 in the red channel.
    fn numbered() -> ColorImage {
        let px: Vec<u8> = (0..6u8).flat_map(|i| [i, 0, 0]).collect();
        ColorImage::from_rgb([3, 2], &px)
    }

    fn reds(img: &ColorImage) -> Vec<u8> {
        img.pixels.iter().map(|p| p.r()).collect()
    }

    #[test]
    fn the_rotated_image_agrees_with_the_rect_mapping() {
        // The pixel at the top-left corner of a rect must land inside the mapped rect.
        use crate::crop::to_view;
        let img = numbered();
        let frame = [0.0, 0.0, 1.0 / 3.0, 0.5]; // pixel 0 only
        for deg in [90, 180, 270] {
            let r = rotate_image(&img, deg);
            let v = to_view(frame, deg);
            let [w, h] = r.size;
            let (cx, cy) = (
                (v[0] + v[2]) / 2.0 * w as f32,
                (v[1] + v[3]) / 2.0 * h as f32,
            );
            let px = r.pixels[cy as usize * w + cx as usize];
            assert_eq!(px.r(), 0, "{deg}: pixel 0 is inside the mapped frame");
        }
    }

    #[test]
    fn rotating_turns_the_picture_clockwise_and_swaps_the_size() {
        // 0 1 2      3 0
        // 3 4 5  ->  4 1
        //            5 2
        let r = rotate_image(&numbered(), 90);
        assert_eq!(r.size, [2, 3]);
        assert_eq!(reds(&r), [3, 0, 4, 1, 5, 2]);
        let r = rotate_image(&numbered(), 180);
        assert_eq!((r.size, reds(&r)), ([3, 2], vec![5, 4, 3, 2, 1, 0]));
        let r = rotate_image(&numbered(), 270);
        assert_eq!((r.size, reds(&r)), ([2, 3], vec![2, 5, 1, 4, 0, 3]));
        assert_eq!(reds(&rotate_image(&numbered(), 0)), [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn four_quarter_turns_restore_the_picture() {
        let mut img = numbered();
        for _ in 0..4 {
            img = rotate_image(&img, 90);
        }
        assert_eq!((img.size, reds(&img)), ([3, 2], reds(&numbered())));
    }

    #[test]
    fn downscale_averages_blocks_and_handles_ragged_edges() {
        // A 2x2 checker of black and white becomes one mid-grey sample.
        let checker = RawImage::new(2, 2, 1, vec![0, 65535, 65535, 0]).unwrap();
        let d = downscale(&checker, 1);
        assert_eq!((d.width, d.height, d.data[0]), (1, 1, 32768));
        // 3 columns at step 2: the last block has only one column and is not diluted.
        let img = RawImage::new(3, 1, 1, vec![100, 300, 900]).unwrap();
        let d = downscale(&img, 2);
        assert_eq!((d.width, d.data.clone()), (2, vec![200, 900]));
        // Colour channels stay separate.
        let rgb = RawImage::new(2, 1, 3, vec![0, 100, 200, 100, 300, 400]).unwrap();
        assert_eq!(downscale(&rgb, 1).data, vec![50, 200, 300]);
    }

    #[test]
    fn downscale_keeps_16_bit_samples() {
        let img = RawImage::new(100, 50, 3, vec![40000; 100 * 50 * 3]).unwrap();
        let d = downscale(&img, 25);
        assert_eq!((d.width, d.height), (25, 13));
        assert!(d.data.iter().all(|&v| v == 40000));
        assert_eq!(downscale(&img, 200), img);
    }

    #[test]
    fn render_negative_flips_brightness_using_the_reference_frame() {
        let img = three_by_one([[60000; 3], [30000; 3], [10000; 3]]);
        let look = Look {
            tone: ToneOptions {
                negative: true,
                auto_levels: false,
                ..ToneOptions::default()
            },
            gray: false,
            rotate: 0,
        };
        let c = render(&img, look, [0.0, 0.0, 1.0, 1.0]);
        let l = |i: usize| c.pixels[i].r();
        assert!(l(0) < l(1) && l(1) < l(2), "dense areas become bright");
    }

    #[test]
    fn render_gray_gives_equal_channels() {
        let img = three_by_one([[65535, 0, 0], [0, 65535, 0], [0, 0, 65535]]);
        let c = render(
            &img,
            Look {
                gray: true,
                ..Look::default()
            },
            [0.0, 0.0, 1.0, 1.0],
        );
        assert!(c.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b()));
    }

    #[test]
    fn render_without_options_matches_the_plain_conversion() {
        let img = three_by_one([[0, 0, 0], [32768, 32768, 32768], [65535, 65535, 65535]]);
        let c = render(&img, Look::default(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(c.pixels, to_color_image(&img, 100).pixels);
    }

    #[test]
    fn downscales_to_max_side() {
        let img = RawImage::new(100, 50, 3, vec![0; 100 * 50 * 3]).unwrap();
        let c = to_color_image(&img, 25);
        assert_eq!(c.size, [25, 13]);
    }
}
