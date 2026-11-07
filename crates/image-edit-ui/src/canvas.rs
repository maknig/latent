//! A picture in a pane with wheel zoom, panning and double-click to fit; shared by the scan
//! preview and the image editor.

use crate::crop;
use crate::view::{self, ZoomState};
use egui::{Color32, Rect, Response, TextureId, Ui, Vec2};

/// The pane's size for the space available: never smaller than a usable minimum.
pub fn pane_size(avail: Vec2) -> Vec2 {
    egui::vec2(avail.x.max(100.0), avail.y.max(100.0))
}

/// What the pane shows.
pub struct Canvas {
    /// The pane's rectangle on the screen.
    pub rect: Rect,
    pub resp: Response,
    /// The part of the picture shown, normalized in view space (see [`view`]).
    pub window: view::Window,
    /// The pane's shape relative to the picture's (see [`view::shape`]), for zoom buttons.
    pub shape: f32,
}

/// Draws the texture (`pic_aspect` = width / height of the picture as displayed) in a pane of
/// `avail` size. The pane has the same size whether the picture is fitted or zoomed (so the
/// layout around it never moves); fitted, the picture is simply centred in it by the zoom
/// window.
/// `prepare` runs once the pane's shape is known (to move the zoom, e.g. to follow a frame).
/// Middle and right drags always pan; `pan_primary` lets the primary button pan as well (for
/// hosts without a tool that needs the primary drag).
pub fn show(
    ui: &mut Ui,
    avail: Vec2,
    tex: TextureId,
    pic_aspect: f32,
    zoom: &mut Option<ZoomState>,
    pan_primary: bool,
    prepare: impl FnOnce(&mut Option<ZoomState>, f32),
) -> Canvas {
    let size = pane_size(avail);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let shape = view::shape(rect.width() / rect.height(), pic_aspect);
    prepare(zoom, shape);
    let window_of = |zoom: &Option<ZoomState>| zoom.unwrap_or_else(ZoomState::fitted).window(shape);

    if let Some(pointer) = resp.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = pinch * (scroll * 0.0025).exp();
        if (factor - 1.0).abs() > 1e-4 {
            let n = crop::from_screen(rect, window_of(zoom), pointer);
            let anchor = [n[0].clamp(0.0, 1.0), n[1].clamp(0.0, 1.0)];
            view::wheel(zoom, factor, anchor, shape);
        }
    }
    if resp.dragged_by(egui::PointerButton::Middle)
        || resp.dragged_by(egui::PointerButton::Secondary)
        || (pan_primary && resp.dragged_by(egui::PointerButton::Primary))
    {
        let d = crop::screen_delta(rect, window_of(zoom), resp.drag_delta());
        view::pan(zoom, [-d[0], -d[1]]);
    }
    if resp.double_clicked() {
        *zoom = None;
    }
    let window = window_of(zoom);

    // Only the part of the picture inside the window is drawn; the rest is background.
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(22));
    let uv = view::visible(window);
    let dst = Rect::from_min_max(
        crop::to_screen(rect, window, [uv[0], uv[1]]),
        crop::to_screen(rect, window, [uv[2], uv[3]]),
    );
    painter.image(
        tex,
        dst,
        Rect::from_min_max(egui::pos2(uv[0], uv[1]), egui::pos2(uv[2], uv[3])),
        Color32::WHITE,
    );
    Canvas {
        rect,
        resp,
        window,
        shape,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws the canvas headless in a panel of `panel` size and returns it.
    fn draw(panel: Vec2, aspect: f32, zoom: &mut Option<ZoomState>) -> Canvas {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                panel + egui::vec2(40.0, 40.0),
            )),
            ..Default::default()
        };
        let mut out = None;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let avail = ui.available_size();
                out = Some(show(
                    ui,
                    avail,
                    TextureId::default(),
                    aspect,
                    zoom,
                    false,
                    |_, _| {},
                ));
            });
        });
        out.unwrap()
    }

    #[test]
    fn the_pane_has_the_same_size_fitted_and_zoomed() {
        let panel = egui::vec2(700.0, 500.0);
        for aspect in [0.4, 1.0, 1.5, 3.0] {
            let fitted = draw(panel, aspect, &mut None).rect.size();
            let mut zoomed = Some(ZoomState {
                center: [0.4, 0.6],
                zoom: 3.0,
                follow: None,
            });
            assert_eq!(
                fitted,
                draw(panel, aspect, &mut zoomed).rect.size(),
                "aspect {aspect}"
            );
            let mut barely = Some(ZoomState {
                center: [0.5, 0.5],
                zoom: 1.05,
                follow: None,
            });
            assert_eq!(
                fitted,
                draw(panel, aspect, &mut barely).rect.size(),
                "aspect {aspect}"
            );
            assert!(
                fitted.x > 600.0 && fitted.y > 400.0,
                "it fills the panel, not the picture"
            );
        }
    }

    #[test]
    fn a_fitted_picture_is_centred_in_the_pane() {
        for aspect in [0.5, 2.0] {
            let c = draw(egui::vec2(700.0, 500.0), aspect, &mut None);
            let center = [
                (c.window[0] + c.window[2]) / 2.0,
                (c.window[1] + c.window[3]) / 2.0,
            ];
            assert!(
                (center[0] - 0.5).abs() < 1e-4 && (center[1] - 0.5).abs() < 1e-4,
                "{center:?}"
            );
            // The whole picture is inside the window; the shorter side fits exactly.
            let (w, h) = (c.window[2] - c.window[0], c.window[3] - c.window[1]);
            assert!(w >= 1.0 - 1e-4 && h >= 1.0 - 1e-4, "{w} x {h}");
            assert!((w.min(h) - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn the_minimum_size_keeps_a_tiny_panel_usable() {
        assert_eq!(pane_size(egui::vec2(20.0, 30.0)), egui::vec2(100.0, 100.0));
        assert_eq!(
            pane_size(egui::vec2(640.0, 480.0)),
            egui::vec2(640.0, 480.0)
        );
    }
}
