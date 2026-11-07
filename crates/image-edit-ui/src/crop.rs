//! Crop frame on the preview: a normalized `[left, top, right, bottom]` rectangle that the user
//! drags (new / move / resize edges), and its mapping to millimetres on the scanner bed.
//! Same interaction model as the crop tool in latent's image view.

use egui::{Color32, CursorIcon, Pos2, Rect, Response, Stroke, Ui};

pub type Crop = [f32; 4];
pub const FULL: Crop = [0.0, 0.0, 1.0, 1.0];

/// Smallest crop edge length (fraction of the preview).
pub const MIN: f32 = 0.02;
/// Pointer distance (px) at which an edge or corner is grabbed.
const HIT: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grab {
    New([f32; 2]),
    /// Which of left, top, right, bottom follow the pointer.
    Edges([bool; 4]),
    Move,
}

/// A normalized rect as seen after turning the picture clockwise by `deg` (0, 90, 180, 270).
pub fn to_view(c: Crop, deg: u16) -> Crop {
    match deg % 360 {
        90 => [1.0 - c[3], c[0], 1.0 - c[1], c[2]],
        180 => [1.0 - c[2], 1.0 - c[3], 1.0 - c[0], 1.0 - c[1]],
        270 => [c[1], 1.0 - c[2], c[3], 1.0 - c[0]],
        _ => c,
    }
}

/// Inverse of [`to_view`]: a rect drawn on the turned picture, back in the original one.
pub fn from_view(c: Crop, deg: u16) -> Crop {
    to_view(c, (360 - deg % 360) % 360)
}

/// Decides what a drag starting at `p` (screen) / `np` (normalized) grabs.
pub fn pick_grab(crop: Rect, p: Pos2, np: [f32; 2]) -> Grab {
    let near_y = p.y >= crop.top() - HIT && p.y <= crop.bottom() + HIT;
    let near_x = p.x >= crop.left() - HIT && p.x <= crop.right() + HIT;
    let edges = [
        near_y && (p.x - crop.left()).abs() <= HIT,
        near_x && (p.y - crop.top()).abs() <= HIT,
        near_y && (p.x - crop.right()).abs() <= HIT,
        near_x && (p.y - crop.bottom()).abs() <= HIT,
    ];
    if edges.iter().any(|&e| e) {
        Grab::Edges(edges)
    } else if crop.contains(p) {
        Grab::Move
    } else {
        Grab::New(np)
    }
}

/// Applies a drag to `crop`. `np` is the pointer position and `delta` the pointer movement
/// since the last frame, both normalized to the preview.
pub fn drag(crop: &mut Crop, grab: Grab, np: [f32; 2], delta: [f32; 2]) {
    match grab {
        Grab::New(a) => {
            let (x0, x1) = (a[0].min(np[0]), a[0].max(np[0]));
            let (y0, y1) = (a[1].min(np[1]), a[1].max(np[1]));
            *crop = [
                x0.min(1.0 - MIN),
                y0.min(1.0 - MIN),
                x1.max(x0 + MIN).min(1.0),
                y1.max(y0 + MIN).min(1.0),
            ];
        }
        Grab::Edges(e) => {
            if e[0] {
                crop[0] = np[0].clamp(0.0, crop[2] - MIN);
            }
            if e[1] {
                crop[1] = np[1].clamp(0.0, crop[3] - MIN);
            }
            if e[2] {
                crop[2] = np[0].clamp(crop[0] + MIN, 1.0);
            }
            if e[3] {
                crop[3] = np[1].clamp(crop[1] + MIN, 1.0);
            }
        }
        Grab::Move => {
            let dx = delta[0].clamp(-crop[0], 1.0 - crop[2]);
            let dy = delta[1].clamp(-crop[1], 1.0 - crop[3]);
            *crop = [crop[0] + dx, crop[1] + dy, crop[2] + dx, crop[3] + dy];
        }
    }
}

fn cursor(grab: Grab) -> CursorIcon {
    match grab {
        Grab::New(_) => CursorIcon::Crosshair,
        Grab::Move => CursorIcon::Grab,
        Grab::Edges([l, t, r, b]) => match ((l, r), (t, b)) {
            ((false, false), _) => CursorIcon::ResizeVertical,
            (_, (false, false)) => CursorIcon::ResizeHorizontal,
            _ if (l && t) || (r && b) => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        },
    }
}

/// A new frame next to `frames[active]`: same size, shifted along the longer side by one frame
/// length (a film strip's frames follow each other), kept inside the preview. Without room on
/// that side it goes to the other side of the active frame.
pub fn next_frame(frames: &[Crop], active: usize) -> Crop {
    let Some(&c) = frames.get(active) else {
        return FULL;
    };
    let (w, h) = (c[2] - c[0], c[3] - c[1]);
    let gap = 0.01;
    let shift = |lo: f32, len: f32| {
        let after = lo + len + gap;
        if after + len <= 1.0 {
            after
        } else {
            (lo - len - gap).max(0.0)
        }
    };
    let (x, y) = if h >= w {
        (c[0], shift(c[1], h))
    } else {
        (shift(c[0], w), c[1])
    };
    [x, y, (x + w).min(1.0), (y + h).min(1.0)]
}

/// The topmost frame other than `active` that contains the normalized point `np`.
pub fn hit_frame(frames: &[Crop], active: usize, np: [f32; 2]) -> Option<usize> {
    frames.iter().enumerate().rev().find_map(|(i, f)| {
        let inside = np[0] >= f[0] && np[0] <= f[2] && np[1] >= f[1] && np[1] <= f[3];
        (i != active && inside).then_some(i)
    })
}

/// A point of the picture (normalized in view space) on the screen, for a pane `rect` that shows
/// the part `window` of the picture.
pub fn to_screen(rect: Rect, window: Crop, n: [f32; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + (n[0] - window[0]) / (window[2] - window[0]) * rect.width(),
        rect.top() + (n[1] - window[1]) / (window[3] - window[1]) * rect.height(),
    )
}

/// Inverse of [`to_screen`]; not limited to the picture.
pub fn from_screen(rect: Rect, window: Crop, p: Pos2) -> [f32; 2] {
    [
        window[0] + (p.x - rect.left()) / rect.width() * (window[2] - window[0]),
        window[1] + (p.y - rect.top()) / rect.height() * (window[3] - window[1]),
    ]
}

/// A pointer movement in pixels as a movement of the picture (normalized), for a pane `rect`
/// that shows `window`.
pub fn screen_delta(rect: Rect, window: Crop, d: egui::Vec2) -> [f32; 2] {
    [
        d.x / rect.width() * (window[2] - window[0]),
        d.y / rect.height() * (window[3] - window[1]),
    ]
}

/// Handles input for the frames over `rect` (the pane) and draws them; `window` is the part of
/// the picture the pane shows (`[0, 0, 1, 1]` for all of it). Only the active frame can be
/// reshaped; pressing on another frame selects it and moves it.
#[allow(clippy::too_many_arguments)]
pub fn overlay(
    ui: &Ui,
    resp: &Response,
    rect: Rect,
    window: Crop,
    frames: &mut [Crop],
    active: &mut usize,
    grab: &mut Option<Grab>,
    accent: Color32,
) {
    if frames.is_empty() {
        return;
    }
    *active = (*active).min(frames.len() - 1);
    let screen = |c: Crop| {
        Rect::from_min_max(
            to_screen(rect, window, [c[0], c[1]]),
            to_screen(rect, window, [c[2], c[3]]),
        )
    };
    // Edits are limited to the picture, not to the part that happens to be visible.
    let norm = |p: Pos2| {
        let n = from_screen(rect, window, p);
        [n[0].clamp(0.0, 1.0), n[1].clamp(0.0, 1.0)]
    };

    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            let g = pick_grab(screen(frames[*active]), p, norm(p));
            *grab = Some(match (g, hit_frame(frames, *active, norm(p))) {
                (Grab::New(_), Some(other)) => {
                    *active = other;
                    Grab::Move
                }
                _ => g,
            });
        }
    }
    if let (true, Some(g), Some(p)) = (resp.dragged(), *grab, resp.interact_pointer_pos()) {
        let d = screen_delta(rect, window, resp.drag_delta());
        drag(&mut frames[*active], g, norm(p), d);
    }
    if resp.drag_stopped() {
        *grab = None;
    }
    if let Some(p) = resp
        .hover_pos()
        .filter(|_| resp.hovered() || resp.dragged())
    {
        ui.ctx().set_cursor_icon(cursor(
            grab.unwrap_or_else(|| pick_grab(screen(frames[*active]), p, norm(p))),
        ));
    }

    let painter = ui.painter_at(rect);
    let dim = Color32::from_black_alpha(150);
    let c = screen(frames[*active]);
    // Dimming the surroundings would hide the other frames, so only do it for a single frame.
    if frames.len() == 1 && c.intersects(rect) {
        let c = c.intersect(rect);
        let bands = [
            Rect::from_min_max(rect.min, Pos2::new(rect.right(), c.top())),
            Rect::from_min_max(Pos2::new(rect.left(), c.bottom()), rect.max),
            Rect::from_min_max(
                Pos2::new(rect.left(), c.top()),
                Pos2::new(c.left(), c.bottom()),
            ),
            Rect::from_min_max(
                Pos2::new(c.right(), c.top()),
                Pos2::new(rect.right(), c.bottom()),
            ),
        ];
        for band in bands {
            painter.rect_filled(band, 0.0, dim);
        }
    }
    for (i, f) in frames.iter().enumerate().filter(|(i, _)| i != active) {
        let r = screen(*f);
        painter.rect_stroke(
            r,
            0.0,
            Stroke::new(1.5_f32, Color32::from_white_alpha(200)),
            egui::StrokeKind::Inside,
        );
        label(&painter, r, i + 1, Color32::from_white_alpha(200));
    }
    let thin = Stroke::new(1.0_f32, Color32::from_white_alpha(70));
    for i in 1..3 {
        let f = i as f32 / 3.0;
        let x = c.left() + c.width() * f;
        let y = c.top() + c.height() * f;
        painter.line_segment([Pos2::new(x, c.top()), Pos2::new(x, c.bottom())], thin);
        painter.line_segment([Pos2::new(c.left(), y), Pos2::new(c.right(), y)], thin);
    }
    painter.rect_stroke(
        c,
        0.0,
        Stroke::new(2.0_f32, accent),
        egui::StrokeKind::Inside,
    );
    for p in handle_points(c) {
        painter.rect_filled(
            Rect::from_center_size(p, egui::vec2(HANDLE, HANDLE)),
            1.0,
            crate::ACCENT,
        );
    }
    if frames.len() > 1 {
        label(&painter, c, *active + 1, accent);
    }
}

/// Edge length (px) of the square handles on the active frame.
const HANDLE: f32 = 8.0;

/// Where the handles sit: the four corners and the middle of each edge.
fn handle_points(r: Rect) -> [Pos2; 8] {
    [
        r.left_top(),
        r.right_top(),
        r.left_bottom(),
        r.right_bottom(),
        r.center_top(),
        r.center_bottom(),
        r.left_center(),
        r.right_center(),
    ]
}

/// Frame number in the frame's top-left corner.
fn label(painter: &egui::Painter, r: Rect, number: usize, color: Color32) {
    painter.text(
        r.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        number.to_string(),
        egui::FontId::proportional(15.0),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Rect {
        Rect::from_min_max(Pos2::new(100.0, 50.0), Pos2::new(500.0, 250.0))
    }

    #[test]
    fn handles_sit_on_the_corners_and_edge_middles() {
        let r = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(110.0, 70.0));
        let pts = handle_points(r);
        assert_eq!(pts.len(), 8);
        for p in pts {
            // On the outline: one coordinate on an edge, the other within the frame.
            let on_x = p.x == r.left() || p.x == r.right();
            let on_y = p.y == r.top() || p.y == r.bottom();
            assert!(on_x || on_y, "{p:?}");
            assert!(r.contains(p));
        }
        assert!(pts.contains(&Pos2::new(60.0, 20.0)) && pts.contains(&Pos2::new(110.0, 45.0)));
        assert!(pts.contains(&r.left_top()) && pts.contains(&r.right_bottom()));
    }

    #[test]
    fn screen_mapping_round_trips_for_any_window() {
        let windows = [FULL, [0.2, 0.3, 0.6, 0.55], [-0.5, -0.1, 1.5, 1.1]];
        for w in windows {
            for n in [[0.0, 0.0], [0.5, 0.5], [0.9, 0.1]] {
                let back = from_screen(pane(), w, to_screen(pane(), w, n));
                assert!((back[0] - n[0]).abs() < 1e-5 && (back[1] - n[1]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn the_window_corners_are_the_pane_corners() {
        let w = [0.2, 0.3, 0.6, 0.55];
        assert_eq!(to_screen(pane(), w, [0.2, 0.3]), pane().min);
        assert_eq!(to_screen(pane(), w, [0.6, 0.55]), pane().max);
    }

    #[test]
    fn a_drag_in_a_zoomed_window_moves_by_the_right_part_of_the_picture() {
        // The pane (400 x 200 px) shows a fifth of the picture's width and height: 40 px across
        // are a tenth of the pane, so two hundredths of the picture; 20 px down likewise.
        let w = [0.4, 0.4, 0.6, 0.6];
        let d = screen_delta(pane(), w, egui::vec2(40.0, 20.0));
        assert!(
            (d[0] - 0.02).abs() < 1e-6 && (d[1] - 0.02).abs() < 1e-6,
            "{d:?}"
        );
        assert_eq!(
            screen_delta(pane(), FULL, egui::vec2(400.0, 200.0)),
            [1.0, 1.0]
        );
    }

    #[test]
    fn view_mapping_turns_a_rect_clockwise() {
        // A rect in the top-left of the picture ends up top-right after 90 degrees clockwise.
        let c = [0.0, 0.0, 0.25, 0.5];
        assert_eq!(to_view(c, 90), [0.5, 0.0, 1.0, 0.25]);
        assert_eq!(to_view(c, 180), [0.75, 0.5, 1.0, 1.0]);
        assert_eq!(to_view(c, 270), [0.0, 0.75, 0.5, 1.0]);
        assert_eq!(to_view(c, 0), c);
        assert_eq!(to_view(c, 360), c);
    }

    #[test]
    fn view_mapping_round_trips_and_four_quarter_turns_are_the_identity() {
        let c = [0.1, 0.2, 0.6, 0.9];
        for deg in [0, 90, 180, 270] {
            let back = from_view(to_view(c, deg), deg);
            for (a, b) in c.iter().zip(back) {
                assert!((a - b).abs() < 1e-6, "{deg}: {c:?} vs {back:?}");
            }
        }
        let mut turned = c;
        for _ in 0..4 {
            turned = to_view(turned, 90);
        }
        for (a, b) in c.iter().zip(turned) {
            assert!((a - b).abs() < 1e-6);
        }
        // Two quarter turns equal one half turn.
        assert_eq!(to_view(to_view(c, 90), 90), to_view(c, 180));
    }

    #[test]
    fn next_frame_follows_along_the_longer_side_and_stays_inside() {
        // A tall frame continues downwards.
        let tall = [0.1, 0.0, 0.2, 0.3];
        let n = next_frame(&[tall], 0);
        assert!((n[2] - n[0] - 0.1).abs() < 1e-6 && (n[3] - n[1] - 0.3).abs() < 1e-6);
        assert!(n[1] > tall[3] - 1e-6 && n[3] <= 1.0);
        // No room below: it goes above instead.
        let low = [0.1, 0.6, 0.2, 0.95];
        let n = next_frame(&[low], 0);
        assert!(n[3] <= low[1] && n[1] >= 0.0);
        // A wide frame continues to the right.
        let wide = [0.0, 0.1, 0.3, 0.2];
        let n = next_frame(&[wide], 0);
        assert!(n[0] > wide[2] - 1e-6 && (n[1] - wide[1]).abs() < 1e-6);
    }

    #[test]
    fn hit_frame_ignores_the_active_one_and_prefers_the_topmost() {
        let frames = [
            [0.0, 0.0, 0.5, 0.5],
            [0.4, 0.4, 0.9, 0.9],
            [0.45, 0.45, 0.6, 0.6],
        ];
        assert_eq!(hit_frame(&frames, 0, [0.1, 0.1]), None);
        assert_eq!(hit_frame(&frames, 0, [0.5, 0.5]), Some(2));
        assert_eq!(hit_frame(&frames, 2, [0.5, 0.5]), Some(1));
        assert_eq!(hit_frame(&frames, 0, [0.95, 0.95]), None);
    }

    #[test]
    fn grab_picks_edge_then_move_then_new() {
        let r = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 200.0));
        assert_eq!(
            pick_grab(r, Pos2::new(103.0, 150.0), [0.0; 2]),
            Grab::Edges([true, false, false, false])
        );
        assert_eq!(
            pick_grab(r, Pos2::new(198.0, 203.0), [0.0; 2]),
            Grab::Edges([false, false, true, true])
        );
        assert_eq!(pick_grab(r, Pos2::new(150.0, 150.0), [0.0; 2]), Grab::Move);
        assert_eq!(
            pick_grab(r, Pos2::new(10.0, 10.0), [0.1, 0.1]),
            Grab::New([0.1, 0.1])
        );
    }

    #[test]
    fn new_crop_normalizes_corners() {
        let mut c = FULL;
        drag(&mut c, Grab::New([0.6, 0.7]), [0.2, 0.3], [0.0; 2]);
        assert_eq!(c, [0.2, 0.3, 0.6, 0.7]);
    }

    #[test]
    fn edges_keep_minimum_size() {
        let mut c = [0.2, 0.2, 0.8, 0.8];
        drag(
            &mut c,
            Grab::Edges([true, false, false, false]),
            [0.95, 0.0],
            [0.0; 2],
        );
        assert!((c[2] - c[0] - MIN).abs() < 1e-6);
    }

    #[test]
    fn move_stays_inside_the_preview() {
        let mut c = [0.2, 0.2, 0.6, 0.6];
        drag(&mut c, Grab::Move, [0.0; 2], [0.9, -0.9]);
        assert!((c[2] - 1.0).abs() < 1e-6 && c[1].abs() < 1e-6);
        assert!((c[2] - c[0] - 0.4).abs() < 1e-6);
    }
}
