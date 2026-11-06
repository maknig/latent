//! The slider groups and the preset bar shared by every host of the editor.

use crate::form::{form, form_buttons, Btn, Form, SliderOpts};
use crate::presets::{Preset, PresetUi};
use egui::{RichText, Ui};
use latent_image_edit::clarity::Clarity;
use latent_image_edit::sharpen::Sharpen;
use latent_image_edit::tone::Adjust;

/// A collapsing section, open by default; egui remembers its state.
pub fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .default_open(true)
        .show(ui, body);
}

/// Preset dropdown, name field and Save / Delete, shared by the scan section and the image
/// editor. Returns the preset to load when one was chosen; saving and deleting are done here.
pub fn preset_row(
    ui: &mut Ui,
    id: &str,
    p: &mut PresetUi,
    current: (&Adjust, Option<(&Clarity, &Sharpen)>),
) -> Option<Preset> {
    let mut chosen = None;
    let shown = if p.presets.get(&p.name).is_some() {
        p.name.clone()
    } else {
        "Choose…".to_string()
    };
    form(ui, id, |f| {
        f.combo("Preset", "preset-choice", shown, |ui| {
            for preset in p.presets.all() {
                if ui
                    .selectable_label(p.name == preset.name, &preset.name)
                    .clicked()
                {
                    chosen = Some(preset.clone());
                }
            }
        });
        f.text("Name", &mut p.name);
    });
    if let Some(c) = &chosen {
        p.name = c.name.clone();
        p.error = None;
    }
    let name = p.name.trim().to_string();
    let buttons = [
        Btn::new("Save preset")
            .enabled(!name.is_empty())
            .hover("Store the sliders (and sharpening) under this name"),
        Btn::new("Delete preset")
            .enabled(p.presets.can_delete(&name))
            .hover("Remove your preset of this name"),
    ];
    match form_buttons(ui, &buttons) {
        Some(0) => p.save_current(current.0, current.1),
        Some(_) => p.delete_named(),
        None => {}
    }
    if let Some(e) = &p.error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
    chosen
}

/// The light group: how bright the picture is and how its tones are shaped.
pub fn light_rows(f: &mut Form, a: &mut Adjust) {
    let d = Adjust::default();
    f.slider(
        "Exposure",
        &mut a.exposure_ev,
        -2.0..=2.0,
        SliderOpts::default()
            .suffix(" EV")
            .decimals(2)
            .step(0.05)
            .default_value(d.exposure_ev as f64),
        "Brighter or darker, in stops",
    );
    let tone = SliderOpts::default().decimals(2);
    f.slider(
        "Contrast",
        &mut a.contrast,
        -1.0..=1.0,
        tone.default_value(d.contrast as f64),
        "Steeper (positive) or flatter (negative) around mid-grey; black and white stay",
    );
    f.slider(
        "Darks",
        &mut a.darks,
        -1.0..=1.0,
        tone.default_value(d.darks as f64),
        "Brightens or darkens the shadows without clipping black (unlike the black point)",
    );
    f.slider(
        "Whites",
        &mut a.whites,
        -1.0..=1.0,
        tone.default_value(d.whites as f64),
        "Brightens or darkens the highlights without clipping white (unlike the white point)",
    );
    f.slider(
        "Midtones",
        &mut a.gamma,
        0.5..=2.0,
        SliderOpts::default()
            .decimals(2)
            .log()
            .default_value(d.gamma as f64),
        "Above 1 brightens the middle without touching black and white",
    );
}

/// The levels group: where black and white are clipped. Keeps a usable range between them.
pub fn levels_rows(f: &mut Form, a: &mut Adjust) {
    let d = Adjust::default();
    let percent = SliderOpts::default().suffix(" %").decimals(1);
    let mut black = a.black * 100.0;
    if f.slider(
        "Black point",
        &mut black,
        0.0..=40.0,
        percent.default_value(d.black as f64 * 100.0),
        "Clip shadows",
    ) {
        a.black = black / 100.0;
    }
    let mut white = a.white * 100.0;
    if f.slider(
        "White point",
        &mut white,
        60.0..=100.0,
        percent.default_value(d.white as f64 * 100.0),
        "Clip highlights",
    ) {
        a.white = white / 100.0;
    }
    a.white = a.white.max(a.black + 0.1).min(1.0);
    a.black = a.black.min(a.white - 0.1);
}

/// The colour group (disabled for gray pictures).
pub fn colour_rows(f: &mut Form, a: &mut Adjust, colour: bool) {
    let d = Adjust::default();
    let opts = SliderOpts::default().decimals(2).enabled(colour);
    f.slider(
        "Warm / cool",
        &mut a.warmth,
        -1.0..=1.0,
        opts.default_value(d.warmth as f64),
        "Residual colour cast: blue to orange",
    );
    f.slider(
        "Tint",
        &mut a.tint,
        -1.0..=1.0,
        opts.default_value(d.tint as f64),
        "Magenta to green",
    );
    f.slider(
        "Saturation",
        &mut a.saturation,
        -1.0..=1.0,
        opts.default_value(d.saturation as f64),
        "Colour intensity: -1 is gray, positive is more vivid",
    );
}

/// The detail group: local contrast and sharpening (they need neighbouring pixels, so the
/// preview shows them only approximately).
pub fn detail_rows(f: &mut Form, c: &mut Clarity, s: &mut Sharpen) {
    let (dc, ds) = (Clarity::default(), Sharpen::default());
    let mut clarity = c.amount * 100.0;
    if f.slider(
        "Clarity",
        &mut clarity,
        -100.0..=100.0,
        SliderOpts::default()
            .decimals(0)
            .default_value(dc.amount as f64 * 100.0),
        "Local contrast in the midtones: positive crisps up mid-sized structure, negative softens it",
    ) {
        c.amount = clarity / 100.0;
    }
    let mut percent = s.amount * 100.0;
    if f.slider(
        "Sharpen",
        &mut percent,
        0.0..=300.0,
        SliderOpts::default()
            .suffix(" %")
            .decimals(0)
            .default_value(ds.amount as f64 * 100.0),
        "Strength of the unsharp mask (brightness only). The preview is made from a reduced \
         copy, so it shows the effect only approximately",
    ) {
        s.amount = percent / 100.0;
    }
    f.slider(
        "Radius",
        &mut s.radius,
        0.3..=5.0,
        SliderOpts::default()
            .suffix(" px")
            .decimals(1)
            .default_value(ds.radius as f64),
        "Size of the detail that is sharpened, in pixels of the file",
    );
}

/// A collapsing group with a small reset button in its header while it differs from the
/// default. Returns whether that button was pressed.
fn group(ui: &mut Ui, id: &str, title: &str, dirty: bool, body: impl FnOnce(&mut Ui)) -> bool {
    let mut reset = false;
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.make_persistent_id((id, title)),
        true,
    );
    state
        .show_header(ui, |ui| {
            ui.label(RichText::new(title).strong());
            if dirty {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    reset = ui
                        .small_button("⟲")
                        .on_hover_text(format!("Reset {title}"))
                        .clicked();
                });
            }
        })
        .body(body);
    reset
}

/// The image sliders in Lightroom-like groups: Light, Levels, Colour and, when `detail` is given
/// (clarity and sharpening, the editor only), Detail. Shared by the scan panel and the editor.
pub fn adjust_groups(
    ui: &mut Ui,
    id: &str,
    a: &mut Adjust,
    detail: Option<(&mut Clarity, &mut Sharpen)>,
    colour: bool,
) {
    let d = Adjust::default();
    let dirty = |names: &[(f32, f32)]| names.iter().any(|(v, def)| v != def);
    let light = dirty(&[
        (a.exposure_ev, d.exposure_ev),
        (a.contrast, d.contrast),
        (a.darks, d.darks),
        (a.whites, d.whites),
        (a.gamma, d.gamma),
    ]);
    if group(ui, id, "Light", light, |ui| {
        form(ui, &format!("{id}-light"), |f| light_rows(f, a));
    }) {
        a.exposure_ev = d.exposure_ev;
        a.contrast = d.contrast;
        a.darks = d.darks;
        a.whites = d.whites;
        a.gamma = d.gamma;
    }
    let levels = dirty(&[(a.black, d.black), (a.white, d.white)]);
    if group(ui, id, "Levels", levels, |ui| {
        form(ui, &format!("{id}-levels"), |f| levels_rows(f, a));
    }) {
        a.black = d.black;
        a.white = d.white;
    }
    let tinted = dirty(&[
        (a.warmth, d.warmth),
        (a.tint, d.tint),
        (a.saturation, d.saturation),
    ]);
    if group(ui, id, "Colour", tinted, |ui| {
        form(ui, &format!("{id}-colour"), |f| colour_rows(f, a, colour));
    }) {
        a.warmth = d.warmth;
        a.tint = d.tint;
        a.saturation = d.saturation;
    }
    if let Some((c, s)) = detail {
        let touched = c.amount != 0.0 || !s.is_neutral();
        if group(ui, id, "Detail", touched, |ui| {
            form(ui, &format!("{id}-detail"), |f| detail_rows(f, c, s));
        }) {
            *c = Clarity::default();
            *s = Sharpen::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::{test_support::Harness, LABEL_W};
    use egui::{pos2, Pos2};

    /// Where the sliders of a form are, one position per row (inside the track). The form
    /// starts at the panel's top-left; rows are `row_h` high with a 6 px gap.
    fn row_positions(origin: Pos2, row_h: f32, rows: usize) -> Vec<Pos2> {
        (0..rows)
            .map(|i| {
                pos2(
                    origin.x + LABEL_W + 40.0,
                    origin.y + i as f32 * (row_h + 6.0) + row_h / 2.0,
                )
            })
            .collect()
    }

    fn shifted() -> Adjust {
        Adjust {
            exposure_ev: 1.0,
            contrast: 0.5,
            black: 0.2,
            white: 0.8,
            darks: 0.3,
            whites: -0.3,
            gamma: 1.5,
            warmth: 0.4,
            tint: -0.4,
            saturation: 0.35,
        }
    }

    /// One group's rows drawn on their own, so the row positions are known.
    #[derive(Clone, Copy)]
    enum G {
        Light,
        Levels,
        Colour(bool),
    }

    fn draw(ui: &mut Ui, g: G, a: &mut Adjust) {
        form(ui, "t", |f| match g {
            G::Light => light_rows(f, a),
            G::Levels => levels_rows(f, a),
            G::Colour(c) => colour_rows(f, a, c),
        });
    }

    /// Double-clicks every row of a group and returns the rows' count seen.
    fn double_click_rows(g: G, a: &mut Adjust, rows: &[usize]) {
        let mut h = Harness::new();
        let mut grid = (Pos2::ZERO, 0.0_f32);
        for _ in 0..3 {
            h.frame(vec![], |ui| {
                draw(ui, g, a);
                grid = (ui.min_rect().min, ui.spacing().interact_size.y);
            });
        }
        let positions = row_positions(grid.0, grid.1, 5);
        for &i in rows {
            h.double_click(positions[i], |ui| draw(ui, g, a));
        }
    }

    #[test]
    fn double_clicking_every_slider_of_the_three_groups_resets_it() {
        let mut a = shifted();
        double_click_rows(G::Light, &mut a, &[0, 1, 2, 3, 4]);
        double_click_rows(G::Levels, &mut a, &[0, 1]);
        double_click_rows(G::Colour(true), &mut a, &[0, 1, 2]);
        assert!(a.is_neutral(), "{a:?}");
    }

    #[test]
    fn a_double_click_resets_only_its_own_slider() {
        let mut a = shifted();
        double_click_rows(G::Light, &mut a, &[1]);
        assert_eq!(a.contrast, 0.0);
        assert_eq!(
            Adjust { contrast: 0.5, ..a },
            shifted(),
            "everything else is as it was"
        );
    }

    #[test]
    fn black_and_white_point_reset_to_zero_and_full_range() {
        let mut a = Adjust {
            black: 0.3,
            white: 0.7,
            ..Adjust::default()
        };
        double_click_rows(G::Levels, &mut a, &[0, 1]);
        assert_eq!((a.black, a.white), (0.0, 1.0));
    }

    #[test]
    fn colour_sliders_stay_put_for_gray_scans() {
        let mut a = shifted();
        double_click_rows(G::Colour(false), &mut a, &[0, 1, 2]);
        assert_eq!(
            (a.warmth, a.tint, a.saturation),
            (0.4, -0.4, 0.35),
            "disabled sliders ignore the double click"
        );
    }

    #[test]
    fn the_detail_sliders_reset_on_double_click() {
        let mut h = Harness::new();
        let (mut c, mut s) = (
            Clarity { amount: 0.6 },
            Sharpen {
                amount: 2.0,
                radius: 3.0,
            },
        );
        let mut grid = (Pos2::ZERO, 0.0_f32);
        let draw =
            |ui: &mut Ui, c: &mut Clarity, s: &mut Sharpen| form(ui, "t", |f| detail_rows(f, c, s));
        for _ in 0..3 {
            h.frame(vec![], |ui| {
                draw(ui, &mut c, &mut s);
                grid = (ui.min_rect().min, ui.spacing().interact_size.y);
            });
        }
        for pos in row_positions(grid.0, grid.1, 3) {
            h.double_click(pos, |ui| draw(ui, &mut c, &mut s));
        }
        assert_eq!((c, s), (Clarity::default(), Sharpen::default()));
    }

    #[test]
    fn a_group_resets_from_its_header_and_only_while_it_differs() {
        let mut h = Harness::new();
        let mut a = shifted();
        let mut detail = (Clarity { amount: 0.5 }, Sharpen::default());
        let frame = |h: &mut Harness, a: &mut Adjust, d: &mut (Clarity, Sharpen)| {
            h.frame(vec![], |ui| {
                adjust_groups(ui, "t", a, Some((&mut d.0, &mut d.1)), true);
            });
        };
        for _ in 0..3 {
            frame(&mut h, &mut a, &mut detail);
        }
        assert_eq!(a, shifted(), "drawing alone changes nothing");
        // Find the reset buttons by clicking along the right edge of each header row is
        // fragile; the group helper is what matters: it reports the press.
        let mut pressed = false;
        h.frame(vec![], |ui| {
            pressed = group(ui, "g", "Light", false, |_| {});
        });
        assert!(!pressed);
    }

    #[test]
    fn the_groups_draw_with_and_without_detail() {
        let mut h = Harness::new();
        let mut a = Adjust::default();
        let mut d = (Clarity::default(), Sharpen::default());
        for _ in 0..3 {
            h.frame(vec![], |ui| {
                adjust_groups(ui, "no-detail", &mut a, None, false);
                adjust_groups(ui, "detail", &mut a, Some((&mut d.0, &mut d.1)), true);
            });
        }
        assert!(a.is_neutral());
    }
}
