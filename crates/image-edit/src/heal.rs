//! Dust and scratch removal: each marked spot is filled by copying the nearby texture that best
//! continues its surroundings (grain included), feathered at the edge, with a flat median fill
//! where no such area exists.
//!
//! Port of `heal` in latent's `images.rs` (8-bit RGB) to 16-bit gray / RGB `RawImage`s; keep the
//! two in sync (same constants and search rules) so they can share one implementation later.

use crate::Stage;
use crate::{RawImage, Result};

/// A spot to heal: centre as fractions of the image width and height, radius as a fraction of
/// the width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// Spots are blended over `FEATHER` x their radius, with a full-strength core.
const FEATHER: f32 = 1.2;
const MAX: f32 = u16::MAX as f32;

/// Heals the spots in the order given; later ones may use the healed pixels as context.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HealStage(pub Vec<Spot>);

impl Stage for HealStage {
    fn apply(&self, mut img: RawImage) -> Result<RawImage> {
        heal(&mut img, &self.0);
        Ok(img)
    }
}

fn at(im: &RawImage, x: i64, y: i64, c: usize) -> f32 {
    let ch = im.channels as usize;
    im.data[(y as usize * im.width as usize + x as usize) * ch + c] as f32
}

/// Pixels covered by any spot (including the feathered edge).
fn spot_mask(w: u32, h: u32, spots: &[Spot]) -> Vec<bool> {
    let mut mask = vec![false; w as usize * h as usize];
    for s in spots {
        let reach = (s.r * w as f32).max(1.0) * FEATHER;
        mark(&mut mask, w, h, s.x * w as f32, s.y * h as f32, reach, true);
    }
    mask
}

fn mark(mask: &mut [bool], w: u32, h: u32, cx: f32, cy: f32, reach: f32, value: bool) {
    let (w, h) = (w as i64, h as i64);
    for y in ((cy - reach).floor() as i64).max(0)..=((cy + reach).ceil() as i64).min(h - 1) {
        for x in ((cx - reach).floor() as i64).max(0)..=((cx + reach).ceil() as i64).min(w - 1) {
            if (x as f32 - cx).hypot(y as f32 - cy) <= reach {
                mask[(y * w + x) as usize] = value;
            }
        }
    }
}

/// Per-channel median of a ring of pixels around the spot: the flat fallback when no texture
/// can be copied.
fn median_fill(im: &RawImage, cx: f32, cy: f32, r: f32) -> Option<Vec<f32>> {
    const ANGLES: usize = 24;
    let (w, h) = (im.width as i64, im.height as i64);
    let mut samples: Vec<Vec<u16>> = vec![vec![]; im.channels as usize];
    for ring in [1.4_f32, 1.7, 2.0] {
        for a in 0..ANGLES {
            let t = (a as f32 + ring) / ANGLES as f32 * std::f32::consts::TAU;
            let (x, y) = (
                (cx + t.cos() * r * ring).round() as i64,
                (cy + t.sin() * r * ring).round() as i64,
            );
            if (0..w).contains(&x) && (0..h).contains(&y) {
                for (c, s) in samples.iter_mut().enumerate() {
                    s.push(at(im, x, y, c) as u16);
                }
            }
        }
    }
    if samples[0].len() < 8 {
        return None;
    }
    Some(
        samples
            .iter_mut()
            .map(|v| {
                v.sort_unstable();
                v[v.len() / 2] as f32
            })
            .collect(),
    )
}

/// Offsets (on a grid of `stride`) of the pixels whose distance from the centre is within
/// `lo..=hi`.
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

/// Offset `(dx, dy)` of the nearby area that looks most like the surroundings of the spot at
/// `(cx, cy)`, and the per-channel mean difference (surroundings minus source) for matching
/// brightness. `mask` marks pixels that must not be used (spots still to be healed).
fn best_source(
    im: &RawImage,
    mask: &[bool],
    cx: f32,
    cy: f32,
    r: f32,
) -> Option<((i64, i64), Vec<f32>)> {
    let (w, h) = (im.width as i64, im.height as i64);
    let chans = im.channels as usize;
    let inner = r * FEATHER;
    let outer = inner + (r * 0.6).max(3.0);
    let stride = ((r / 6.0) as i64).max(1);
    let usable =
        |x: i64, y: i64| (0..w).contains(&x) && (0..h).contains(&y) && !mask[(y * w + x) as usize];
    let (px, py) = (cx.round() as i64, cy.round() as i64);
    // Context: pixels around the spot that are themselves intact. Disc: what a candidate must
    // supply.
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
                for c in 0..chans {
                    let d =
                        (at(im, px + x, py + y, c) - at(im, px + dx + x, py + dy + y, c)) as f64;
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
    let mut delta = vec![0.0_f32; chans];
    for &(x, y) in &context {
        for (c, d) in delta.iter_mut().enumerate() {
            *d += (at(im, px + x, py + y, c) - at(im, px + dx + x, py + dy + y, c))
                / context.len() as f32;
        }
    }
    Some(((dx, dy), delta))
}

/// Heals each spot by copying the nearby texture that best continues its surroundings (grain
/// included), feathered at the edge; falls back to a flat median fill where no such area
/// exists. Spots are healed in order and later ones may use the healed pixels as context.
pub fn heal(im: &mut RawImage, spots: &[Spot]) {
    if spots.is_empty() {
        return;
    }
    let (w, h) = (im.width as i64, im.height as i64);
    let chans = im.channels as usize;
    let mut mask = spot_mask(im.width, im.height, spots);
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
                    let fill: Vec<f32> = match (&source, &flat) {
                        (Some(((dx, dy), delta)), _) => {
                            let (sx, sy) = (x + dx, y + dy);
                            if !(0..w).contains(&sx) || !(0..h).contains(&sy) {
                                continue;
                            }
                            (0..chans).map(|c| at(im, sx, sy, c) + delta[c]).collect()
                        }
                        (None, Some(f)) => f.clone(),
                        _ => continue,
                    };
                    let base = (y as usize * w as usize + x as usize) * chans;
                    for (c, f) in fill.iter().enumerate() {
                        let p = &mut im.data[base + c];
                        *p = (*p as f32 * (1.0 - a) + f * a).round().clamp(0.0, MAX) as u16;
                    }
                }
            }
        }
        // The healed area is intact now and may serve as context for later spots.
        mark(&mut mask, im.width, im.height, cx, cy, r * FEATHER, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth gradient with a deterministic grain, in `channels` planes.
    fn texture(w: u32, h: u32, channels: u8) -> RawImage {
        let mut data = vec![];
        for y in 0..h {
            for x in 0..w {
                for c in 0..channels as u32 {
                    let grain = ((x * 7919 + y * 104_729 + c * 31) % 997) as f32 / 997.0 - 0.5;
                    let v =
                        20000.0 + 30000.0 * x as f32 / w as f32 + 4000.0 * grain + 500.0 * c as f32;
                    data.push(v as u16);
                }
            }
        }
        RawImage::new(w, h, channels, data).unwrap()
    }

    fn speck(img: &mut RawImage, cx: i64, cy: i64, radius: i64, value: u16) {
        let ch = img.channels as usize;
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                if (x - cx).pow(2) + (y - cy).pow(2) <= radius * radius {
                    let base = (y as usize * img.width as usize + x as usize) * ch;
                    img.data[base..base + ch].fill(value);
                }
            }
        }
    }

    /// Mean absolute difference over the disc of `radius` around `(cx, cy)`.
    fn disc_error(a: &RawImage, b: &RawImage, cx: i64, cy: i64, radius: i64) -> f64 {
        let ch = a.channels as usize;
        let (mut sum, mut n) = (0.0, 0.0);
        for y in cy - radius..=cy + radius {
            for x in cx - radius..=cx + radius {
                if (x - cx).pow(2) + (y - cy).pow(2) <= radius * radius {
                    let base = (y as usize * a.width as usize + x as usize) * ch;
                    for c in 0..ch {
                        sum += (a.data[base + c] as f64 - b.data[base + c] as f64).abs();
                        n += 1.0;
                    }
                }
            }
        }
        sum / n
    }

    fn check(channels: u8) {
        let clean = texture(200, 150, channels);
        let mut dirty = clean.clone();
        speck(&mut dirty, 100, 75, 4, 65535);
        let before = disc_error(&dirty, &clean, 100, 75, 4);
        let mut healed = dirty.clone();
        heal(
            &mut healed,
            &[Spot {
                x: 100.0 / 200.0,
                y: 75.0 / 150.0,
                r: 5.0 / 200.0,
            }],
        );
        let after = disc_error(&healed, &clean, 100, 75, 4);
        assert!(
            after < before / 4.0,
            "speck stays: error {before:.0} -> {after:.0}"
        );
        // Far away from the spot nothing changes.
        assert_eq!(
            &healed.data[..200 * channels as usize],
            &dirty.data[..200 * channels as usize]
        );
    }

    #[test]
    fn a_speck_on_grain_is_removed_in_gray() {
        check(1);
    }

    #[test]
    fn a_speck_on_grain_is_removed_in_colour() {
        check(3);
    }

    #[test]
    fn no_spots_leave_the_image_identical() {
        let img = texture(50, 40, 1);
        let mut out = img.clone();
        heal(&mut out, &[]);
        assert_eq!(out, img);
    }

    #[test]
    fn spots_at_edges_and_corners_do_not_panic() {
        let mut img = texture(60, 40, 3);
        let spots: Vec<Spot> = [(0.0, 0.0), (1.0, 1.0), (0.0, 0.5), (1.0, 0.0), (0.5, 1.0)]
            .iter()
            .map(|&(x, y)| Spot { x, y, r: 0.05 })
            .collect();
        heal(&mut img, &spots);
        let mut tiny = texture(4, 4, 1);
        heal(
            &mut tiny,
            &[Spot {
                x: 0.5,
                y: 0.5,
                r: 0.5,
            }],
        );
    }

    #[test]
    fn the_sixteen_bit_range_is_kept() {
        // Values far above 8 bit survive: a bright flat area stays bright.
        let mut img = RawImage::new(80, 60, 1, vec![60000; 80 * 60]).unwrap();
        speck(&mut img, 40, 30, 3, 100);
        heal(
            &mut img,
            &[Spot {
                x: 0.5,
                y: 0.5,
                r: 4.0 / 80.0,
            }],
        );
        let centre = img.data[30 * 80 + 40];
        assert!((centre as i32 - 60000).abs() < 300, "centre {centre}");
    }

    #[test]
    fn the_stage_heals_like_the_function() {
        let mut dirty = texture(100, 80, 1);
        speck(&mut dirty, 50, 40, 3, 0);
        let spots = vec![Spot {
            x: 0.5,
            y: 0.5,
            r: 0.04,
        }];
        let mut direct = dirty.clone();
        heal(&mut direct, &spots);
        assert_eq!(HealStage(spots).apply(dirty).unwrap(), direct);
    }
}
