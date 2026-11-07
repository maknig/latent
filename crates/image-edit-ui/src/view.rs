//! Zooming the preview: which part of the (rotated) picture the pane shows.
//!
//! Everything is in *view space*: normalized coordinates of the picture as it is displayed
//! (after rotation), `[left, top, right, bottom]`. The zoom is a centre and a factor; the
//! visible window follows from them and the shape of the pane, so resizing the pane never
//! distorts the picture and zooming out ends exactly at the fitted view (zoom 1).

use crate::crop::Crop;

/// The visible part of the picture, normalized in view space. It may reach beyond `0..=1`
/// where the pane is larger than the picture.
pub type Window = Crop;

/// Largest zoom (the window shows 1/`MAX_ZOOM` of the picture's shorter side).
pub const MAX_ZOOM: f32 = 40.0;
/// Share of the frame's size kept around it when zooming to a frame.
pub const FRAME_MARGIN: f32 = 0.25;

/// What the pane shows when it is not fitted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomState {
    /// Centre of the window in view space, always inside the picture.
    pub center: [f32; 2],
    /// 1 = the whole picture fits the pane; 2 = twice as large, and so on.
    pub zoom: f32,
    /// `(active frame, rotation)` the window was set for by "zoom to frame": while it matches
    /// the view stays; when the active frame or the rotation changes the view moves along.
    /// `None` after the user zoomed or panned by hand.
    pub follow: Option<(usize, u16)>,
}

/// Window sizes for a zoom: the shorter side of the *picture* shown is `1 / zoom`.
/// `shape` is the pane's aspect ratio divided by the picture's.
fn size_for(zoom: f32, shape: f32) -> [f32; 2] {
    let m = 1.0 / zoom;
    if shape >= 1.0 {
        [m * shape, m]
    } else {
        [m, m / shape]
    }
}

fn clamp_center(c: [f32; 2]) -> [f32; 2] {
    [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0)]
}

/// `shape` for a pane and a picture, given as width / height.
pub fn shape(pane_aspect: f32, picture_aspect: f32) -> f32 {
    pane_aspect / picture_aspect.max(1e-6)
}

impl ZoomState {
    /// The whole picture, centred.
    pub fn fitted() -> Self {
        Self {
            center: [0.5, 0.5],
            zoom: 1.0,
            follow: None,
        }
    }

    /// The window to show in a pane of the given `shape`.
    pub fn window(&self, shape: f32) -> Window {
        let [w, h] = size_for(self.zoom, shape);
        [
            self.center[0] - w / 2.0,
            self.center[1] - h / 2.0,
            self.center[0] + w / 2.0,
            self.center[1] + h / 2.0,
        ]
    }

    /// Zooms to `frame` (view space) plus [`FRAME_MARGIN`] of its size on every side. The
    /// window is made to contain all of it.
    pub fn around_frame(frame: Crop, shape: f32) -> Self {
        let (fw, fh) = (frame[2] - frame[0], frame[3] - frame[1]);
        let (w, h) = (
            fw * (1.0 + 2.0 * FRAME_MARGIN),
            fh * (1.0 + 2.0 * FRAME_MARGIN),
        );
        // The shorter side of the picture the window must show.
        let m = if shape >= 1.0 {
            h.max(w / shape)
        } else {
            w.max(h * shape)
        };
        Self {
            center: clamp_center([(frame[0] + frame[2]) / 2.0, (frame[1] + frame[3]) / 2.0]),
            zoom: (1.0 / m.max(1e-6)).clamp(1.0, MAX_ZOOM),
            follow: None,
        }
    }

    /// Zooms by `factor` (above 1 in) keeping the picture point `anchor` under the pointer.
    /// Returns `false` when the result is the fitted view (zoom 1) and the zoom should end.
    pub fn zoom_at(&mut self, factor: f32, anchor: [f32; 2], shape: f32) -> bool {
        let before = self.window(shape);
        let (bw, bh) = (before[2] - before[0], before[3] - before[1]);
        // Where the anchor lies in the window, as a fraction.
        let t = [(anchor[0] - before[0]) / bw, (anchor[1] - before[1]) / bh];
        self.zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
        let [w, h] = size_for(self.zoom, shape);
        self.center = clamp_center([anchor[0] - (t[0] - 0.5) * w, anchor[1] - (t[1] - 0.5) * h]);
        self.follow = None;
        self.zoom > 1.0 + 1e-4
    }

    /// Moves the window by `delta` (view space).
    pub fn pan(&mut self, delta: [f32; 2]) {
        self.center = clamp_center([self.center[0] + delta[0], self.center[1] + delta[1]]);
        self.follow = None;
    }
}

/// Keeps the zoom of "zoom to frame" on the active frame. `frames` are in bed space, `rotate`
/// turns them into view space. Call every frame; does nothing for a free zoom.
pub fn follow_frame(
    zoom: &mut Option<ZoomState>,
    frames: &[Crop],
    active: usize,
    rotate: u16,
    shape: f32,
) {
    let Some(z) = zoom else { return };
    let Some(key) = z.follow else { return };
    if key == (active, rotate) {
        return;
    }
    let Some(&frame) = frames.get(active) else {
        return;
    };
    *z = ZoomState::around_frame(crate::crop::to_view(frame, rotate), shape);
    z.follow = Some((active, rotate));
}

/// Starts following the active frame (the zoom is set on the next [`follow_frame`]).
pub fn start_following() -> ZoomState {
    ZoomState {
        follow: Some((usize::MAX, u16::MAX)),
        ..ZoomState::fitted()
    }
}

/// Mouse wheel / pinch: zoom at `anchor`; the zoom ends when it is back to the fitted view.
pub fn wheel(zoom: &mut Option<ZoomState>, factor: f32, anchor: [f32; 2], shape: f32) {
    let mut z = zoom.unwrap_or_else(ZoomState::fitted);
    *zoom = z.zoom_at(factor, anchor, shape).then_some(z);
}

/// Pans a zoomed view by `delta` (view space); the fitted view cannot be panned.
pub fn pan(zoom: &mut Option<ZoomState>, delta: [f32; 2]) {
    if let Some(z) = zoom {
        z.pan(delta);
    }
}

/// The part of the picture that is visible (`window` clipped to the picture), as the texture
/// UV rectangle to draw.
pub fn visible(window: Window) -> Crop {
    [
        window[0].max(0.0),
        window[1].max(0.0),
        window[2].min(1.0),
        window[3].min(1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: f32 = 1.0;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn size(w: Window) -> (f32, f32) {
        (w[2] - w[0], w[3] - w[1])
    }

    #[test]
    fn the_window_has_the_shape_of_the_pane() {
        for shape in [0.5, 1.0, 2.0, 3.5] {
            let (w, h) = size(ZoomState::fitted().window(shape));
            assert!(approx(w / h, shape), "{shape}: {w} x {h}");
            // At zoom 1 the shorter picture side is exactly shown.
            assert!(approx(w.min(h), 1.0));
        }
    }

    #[test]
    fn the_fitted_window_is_centred_and_reaches_beyond_the_picture_on_one_axis_only() {
        for shape in [0.5, 1.0, 2.0] {
            let w = ZoomState::fitted().window(shape);
            let (sw, sh) = size(w);
            assert!(approx((w[0] + w[2]) / 2.0, 0.5) && approx((w[1] + w[3]) / 2.0, 0.5));
            assert!(sw >= 1.0 - 1e-4 && sh >= 1.0 - 1e-4);
            let beyond = [sw > 1.0 + 1e-4, sh > 1.0 + 1e-4];
            assert!(!(beyond[0] && beyond[1]), "{shape}: {sw} x {sh}");
        }
    }

    #[test]
    fn zooming_shows_less_of_the_picture() {
        let z = ZoomState {
            zoom: 4.0,
            ..ZoomState::fitted()
        };
        let (w, h) = size(z.window(2.0));
        assert!(approx(h, 0.25) && approx(w, 0.5), "{w} x {h}");
    }

    #[test]
    fn zooming_to_a_frame_contains_the_frame_and_its_margin() {
        let frame = [0.30, 0.40, 0.45, 0.60];
        for shape in [0.6, 1.0, 1.7] {
            let z = ZoomState::around_frame(frame, shape);
            let win = z.window(shape);
            let (fw, fh) = (frame[2] - frame[0], frame[3] - frame[1]);
            assert!(
                win[0] <= frame[0] - fw * FRAME_MARGIN + 1e-4,
                "{shape}: {win:?}"
            );
            assert!(
                win[2] >= frame[2] + fw * FRAME_MARGIN - 1e-4,
                "{shape}: {win:?}"
            );
            assert!(
                win[1] <= frame[1] - fh * FRAME_MARGIN + 1e-4,
                "{shape}: {win:?}"
            );
            assert!(
                win[3] >= frame[3] + fh * FRAME_MARGIN - 1e-4,
                "{shape}: {win:?}"
            );
            assert!(z.zoom > 1.0);
            assert_eq!(z.center, [0.375, 0.5]);
        }
    }

    #[test]
    fn a_frame_larger_than_the_picture_gives_the_fitted_view_and_the_zoom_is_capped() {
        assert_eq!(
            ZoomState::around_frame([0.0, 0.0, 1.0, 1.0], SQUARE).zoom,
            1.0
        );
        let tiny = ZoomState::around_frame([0.5, 0.5, 0.5001, 0.5001], SQUARE);
        assert_eq!(tiny.zoom, MAX_ZOOM);
    }

    #[test]
    fn a_frame_at_the_edge_keeps_the_centre_inside_the_picture() {
        let z = ZoomState::around_frame([0.9, 0.9, 1.0, 1.0], SQUARE);
        assert!(z.center.iter().all(|c| (0.0..=1.0).contains(c)));
        assert!(z.zoom > 1.0);
    }

    #[test]
    fn zooming_at_the_pointer_keeps_the_point_under_it() {
        let shape = 1.6;
        let mut z = ZoomState {
            zoom: 2.0,
            center: [0.4, 0.5],
            follow: None,
        };
        let anchor = [0.55, 0.45];
        let before = z.window(shape);
        let t = [
            (anchor[0] - before[0]) / size(before).0,
            (anchor[1] - before[1]) / size(before).1,
        ];
        assert!(z.zoom_at(1.5, anchor, shape));
        let after = z.window(shape);
        let t2 = [
            (anchor[0] - after[0]) / size(after).0,
            (anchor[1] - after[1]) / size(after).1,
        ];
        assert!(
            approx(t[0], t2[0]) && approx(t[1], t2[1]),
            "{t:?} vs {t2:?}"
        );
        assert!(approx(z.zoom, 3.0));
    }

    #[test]
    fn zooming_out_ends_at_the_fitted_view() {
        let mut zoom = None;
        wheel(&mut zoom, 2.0, [0.3, 0.3], SQUARE);
        assert!(zoom.is_some());
        wheel(&mut zoom, 1.5, [0.3, 0.3], SQUARE);
        assert!(approx(zoom.unwrap().zoom, 3.0));
        wheel(&mut zoom, 1.0 / 1.5, [0.3, 0.3], SQUARE);
        wheel(&mut zoom, 1.0 / 2.0, [0.3, 0.3], SQUARE);
        assert!(zoom.is_none(), "back at zoom 1 the zoom state is dropped");
        wheel(&mut zoom, 0.5, [0.5, 0.5], SQUARE);
        assert!(zoom.is_none(), "cannot zoom out of the fitted view");
    }

    #[test]
    fn the_zoom_is_capped_at_the_maximum() {
        let mut zoom = None;
        for _ in 0..30 {
            wheel(&mut zoom, 2.0, [0.5, 0.5], SQUARE);
        }
        assert_eq!(zoom.unwrap().zoom, MAX_ZOOM);
    }

    #[test]
    fn panning_stays_inside_the_picture_and_ends_following() {
        let mut zoom = Some(ZoomState {
            zoom: 3.0,
            center: [0.5, 0.5],
            follow: Some((1, 0)),
        });
        pan(&mut zoom, [0.2, -0.1]);
        let z = zoom.unwrap();
        assert!(approx(z.center[0], 0.7) && approx(z.center[1], 0.4));
        assert_eq!(z.follow, None);
        pan(&mut zoom, [5.0, -5.0]);
        assert_eq!(zoom.unwrap().center, [1.0, 0.0]);
        let mut none = None;
        pan(&mut none, [0.1, 0.1]);
        assert!(none.is_none(), "the fitted view cannot be panned");
    }

    #[test]
    fn the_view_follows_the_active_frame_and_the_rotation_but_not_free_zoom() {
        let frames = [[0.1, 0.1, 0.3, 0.3], [0.6, 0.6, 0.8, 0.8]];
        let mut zoom = Some(start_following());
        follow_frame(&mut zoom, &frames, 0, 0, SQUARE);
        let first = zoom.unwrap();
        assert_eq!(first.follow, Some((0, 0)));
        assert!(approx(first.center[0], 0.2));

        // Same frame again: untouched, even if the user could have edited it meanwhile.
        let edited = [[0.1, 0.1, 0.35, 0.35], frames[1]];
        follow_frame(&mut zoom, &edited, 0, 0, SQUARE);
        assert_eq!(zoom.unwrap(), first);

        follow_frame(&mut zoom, &frames, 1, 0, SQUARE);
        assert!(approx(zoom.unwrap().center[0], 0.7));
        follow_frame(&mut zoom, &frames, 1, 90, SQUARE);
        assert_eq!(zoom.unwrap().follow, Some((1, 90)));

        // After a free zoom nothing follows any more.
        wheel(&mut zoom, 1.1, [0.5, 0.5], SQUARE);
        let free = zoom.unwrap();
        follow_frame(&mut zoom, &frames, 0, 0, SQUARE);
        assert_eq!(zoom.unwrap(), free);
        follow_frame(&mut None, &frames, 0, 0, SQUARE);
    }

    #[test]
    fn the_visible_part_is_the_window_clipped_to_the_picture() {
        assert_eq!(visible([-0.5, 0.2, 1.5, 0.8]), [0.0, 0.2, 1.0, 0.8]);
        assert_eq!(visible([0.2, 0.2, 0.4, 0.4]), [0.2, 0.2, 0.4, 0.4]);
    }

    #[test]
    fn the_shape_accounts_for_the_pictures_own_aspect() {
        assert!(approx(shape(2.0, 2.0), 1.0));
        assert!(approx(shape(2.0, 0.5), 4.0));
        assert!(shape(1.0, 0.0).is_finite());
    }
}
