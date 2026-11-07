//! The dust tool of the image editor: click or drag over dust and scratches to mark spots,
//! which `latent_image_edit::heal` fills when the edits are shown or applied. Modelled on
//! `heal_overlay` in latent's image view.
//!
//! Spots are stored in the *file's* orientation (fractions of its width and height, radius as a
//! fraction of its width) so they stay right when the picture is turned; marking happens on the
//! turned picture, so [`spot_from_view`] / [`spot_to_view`] convert.

use crate::crop::{self, Crop};
use egui::{Color32, CursorIcon, PointerButton, Response, Stroke, Ui};
use latent_image_edit::heal::Spot;

/// Brush radius range as a fraction of the picture's width (as in latent).
pub const BRUSH: std::ops::RangeInclusive<f32> = 0.002..=0.03;
pub const DEFAULT_BRUSH: f32 = 0.008;
/// A new spot is only added when the pointer moved this far (in radii) from the last one.
const SPACING: f32 = 0.7;

/// A spot marked at `view` (normalized on the picture turned clockwise by `deg`) with the
/// radius `r_view` (fraction of the turned picture's width). `aspect` is the file's width / height.
pub fn spot_from_view(view: [f32; 2], r_view: f32, deg: u16, aspect: f32) -> Spot {
    let p = crop::from_view([view[0], view[1], view[0], view[1]], deg);
    let r = if deg.is_multiple_of(180) {
        r_view
    } else {
        r_view / aspect.max(1e-6)
    };
    Spot {
        x: p[0],
        y: p[1],
        r,
    }
}

/// Inverse of [`spot_from_view`]: centre and radius on the turned picture.
pub fn spot_to_view(s: &Spot, deg: u16, aspect: f32) -> ([f32; 2], f32) {
    let p = crop::to_view([s.x, s.y, s.x, s.y], deg);
    let r = if deg.is_multiple_of(180) {
        s.r
    } else {
        s.r * aspect
    };
    ([p[0], p[1]], r)
}

/// Handles marking over the pane `rect` (showing `window`) and draws the spots and the brush.
/// Returns whether the primary button is held down (the preview waits until it is released).
pub fn overlay(
    ui: &Ui,
    resp: &Response,
    rect: egui::Rect,
    window: Crop,
    spots: &mut Vec<Spot>,
    (deg, aspect, brush): (u16, f32, f32),
) -> bool {
    let picture_w = rect.width() / (window[2] - window[0]);
    let radius_px = brush * picture_w;
    let screen = |s: &Spot| {
        let (c, r) = spot_to_view(s, deg, aspect);
        (crop::to_screen(rect, window, c), r * picture_w)
    };
    let pos = resp.interact_pointer_pos().or(resp.hover_pos());
    let primary = resp.dragged_by(PointerButton::Primary);
    if let Some(p) = pos {
        let n = crop::from_screen(rect, window, p);
        let inside = (0.0..=1.0).contains(&n[0]) && (0.0..=1.0).contains(&n[1]);
        let far = spots
            .last()
            .is_none_or(|l| screen(l).0.distance(p) >= radius_px * SPACING);
        let start = resp.drag_started_by(PointerButton::Primary) || resp.clicked();
        if inside && (start || (primary && far)) {
            spots.push(spot_from_view(n, brush, deg, aspect));
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        if let Some(p) = resp.hover_pos() {
            ui.painter_at(rect).circle_stroke(
                p,
                radius_px.max(2.0),
                Stroke::new(1.5_f32, Color32::WHITE),
            );
        }
    }
    let painter = ui.painter_at(rect);
    for s in spots.iter() {
        let (c, r) = screen(s);
        painter.circle_stroke(
            c,
            r.max(2.0),
            Stroke::new(1.5_f32, Color32::from_rgb(255, 80, 80)),
        );
    }
    resp.is_pointer_button_down_on()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_round_trip_through_every_rotation() {
        let aspect = 1.5;
        for deg in [0, 90, 180, 270] {
            let s = Spot {
                x: 0.2,
                y: 0.7,
                r: 0.01,
            };
            let (c, r) = spot_to_view(&s, deg, aspect);
            let back = spot_from_view(c, r, deg, aspect);
            assert!(
                (back.x - s.x).abs() < 1e-5
                    && (back.y - s.y).abs() < 1e-5
                    && (back.r - s.r).abs() < 1e-5,
                "{deg}: {back:?}"
            );
        }
    }

    #[test]
    fn a_turned_picture_marks_the_right_place_in_the_file() {
        // Turned 90 degrees clockwise, the file's top-left corner is at the view's top-right.
        let s = spot_from_view([1.0, 0.0], 0.01, 90, 2.0);
        assert!(s.x.abs() < 1e-6 && s.y.abs() < 1e-6, "{s:?}");
        // The view's width is the file's height, so the radius shrinks by the aspect.
        assert!((s.r - 0.005).abs() < 1e-6);
        // Upright nothing changes.
        let s = spot_from_view([0.3, 0.4], 0.01, 0, 2.0);
        assert_eq!((s.x, s.y, s.r), (0.3, 0.4, 0.01));
    }
}
