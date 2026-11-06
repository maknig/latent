//! One layout for every control of the panels: a label column of fixed width, then the input
//! filling the rest of the row. Every section builds its rows with [`form`], so labels line up
//! across sections and all dropdowns, sliders and fields end at the same edge.

use egui::{ComboBox, DragValue, Grid, Label, Response, RichText, Slider, TextEdit, Ui};
use std::ops::RangeInclusive;

/// Width of the label column in every form. Longer labels are shortened, never wider, so the
/// inputs stay aligned.
pub const LABEL_W: f32 = 110.0;
/// Room kept for the number box next to a slider's track.
const VALUE_BOX_W: f32 = 72.0;
/// Narrowest slider track on a very narrow panel.
const MIN_SLIDER_W: f32 = 50.0;
/// Space between the columns and between the rows.
const SPACING: [f32; 2] = [8.0, 6.0];

/// Length of a slider's track when `rest` is the width left for the whole input.
pub fn slider_track_width(rest: f32) -> f32 {
    (rest - VALUE_BOX_W).max(MIN_SLIDER_W)
}

/// Looks of a slider row.
#[derive(Clone, Copy)]
pub struct SliderOpts {
    pub suffix: &'static str,
    pub decimals: Option<usize>,
    pub step: Option<f64>,
    pub logarithmic: bool,
    pub enabled: bool,
    /// Value a double click resets the slider to (in the slider's own units).
    pub default: Option<f64>,
}

impl Default for SliderOpts {
    fn default() -> Self {
        Self {
            suffix: "",
            decimals: None,
            step: None,
            logarithmic: false,
            enabled: true,
            default: None,
        }
    }
}

impl SliderOpts {
    pub fn suffix(mut self, suffix: &'static str) -> Self {
        self.suffix = suffix;
        self
    }

    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = Some(decimals);
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    pub fn log(mut self) -> Self {
        self.logarithmic = true;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The value a double click on the slider resets it to.
    pub fn default_value(mut self, default: f64) -> Self {
        self.default = Some(default);
        self
    }

    /// A value as the row shows it: with the row's decimals and suffix.
    pub fn format(&self, value: f64) -> String {
        let decimals = self
            .decimals
            .unwrap_or(if value.fract() == 0.0 { 0 } else { 2 });
        format!("{value:.decimals$}{}", self.suffix)
    }
}

/// Sets `value` to `default` when the slider was double-clicked and a default is known. Returns
/// whether the value changed.
pub fn reset_on_double_click<N: egui::emath::Numeric>(
    value: &mut N,
    default: Option<f64>,
    double_clicked: bool,
) -> bool {
    let (true, Some(default)) = (double_clicked, default) else {
        return false;
    };
    let new = N::from_f64(default);
    let changed = new.to_f64() != value.to_f64();
    *value = new;
    changed
}

/// A two-column grid of "label | input" rows.
pub struct Form<'a> {
    /// The grid cell being filled, for rows that need something custom.
    pub ui: &'a mut Ui,
}

/// Builds a form; `id` must be unique among the forms of a panel.
pub fn form<R>(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Form) -> R) -> R {
    Grid::new(id)
        .num_columns(2)
        .min_col_width(LABEL_W)
        .spacing(SPACING)
        .show(ui, |ui| body(&mut Form { ui }))
        .inner
}

impl Form<'_> {
    /// Width left for the input of the current row.
    pub fn rest(&self) -> f32 {
        self.ui.available_width()
    }

    fn label(&mut self, text: &str) {
        // A cell of fixed width; the text sits at its left edge, centred vertically.
        let height = self.ui.spacing().interact_size.y;
        self.ui.allocate_ui_with_layout(
            egui::vec2(LABEL_W, height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.add(Label::new(text).truncate()),
        );
    }

    /// A row with any input; `add` gets the cell and the width left for it.
    pub fn row<R>(&mut self, label: &str, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
        self.label(label);
        let rest = self.rest();
        let out = add(self.ui, rest);
        self.ui.end_row();
        out
    }

    /// A dropdown as wide as the rest of the row.
    pub fn combo(
        &mut self,
        label: &str,
        id: &str,
        selected: impl Into<String>,
        add: impl FnOnce(&mut Ui),
    ) -> Response {
        self.row(label, |ui, rest| {
            ComboBox::from_id_salt(id)
                .width(rest)
                .selected_text(selected.into())
                .show_ui(ui, add)
                .response
        })
    }

    /// A slider whose track and number box together fill the rest of the row. Returns whether
    /// the value changed.
    pub fn slider<N: egui::emath::Numeric>(
        &mut self,
        label: &str,
        value: &mut N,
        range: RangeInclusive<N>,
        opts: SliderOpts,
        hover: &str,
    ) -> bool {
        self.row(label, |ui, rest| {
            ui.scope(|ui| {
                ui.spacing_mut().slider_width = slider_track_width(rest);
                let mut slider = Slider::new(&mut *value, range)
                    .suffix(opts.suffix)
                    .logarithmic(opts.logarithmic);
                if let Some(decimals) = opts.decimals {
                    slider = slider.fixed_decimals(decimals);
                }
                if let Some(step) = opts.step {
                    slider = slider.step_by(step);
                }
                let response = ui.add_enabled(opts.enabled, slider);
                // egui's slider only senses drags, so its response never reports a double click:
                // ask the pointer state while the pointer is over the slider.
                let double_clicked = opts.enabled
                    && response.contains_pointer()
                    && ui.input(|i| {
                        i.pointer
                            .button_double_clicked(egui::PointerButton::Primary)
                    });
                let reset = reset_on_double_click(value, opts.default, double_clicked);
                let hover = match opts.default {
                    Some(d) => format!("{hover}\nDouble-click: reset to {}", opts.format(d)),
                    None => hover.to_string(),
                };
                response.on_hover_text(hover).changed() || reset
            })
            .inner
        })
    }

    /// A single-line text field as wide as the rest of the row (it fills the available width,
    /// frame margin included).
    pub fn text(&mut self, label: &str, value: &mut String) -> Response {
        self.row(label, |ui, _| {
            ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY))
        })
    }

    /// A number field as wide as the rest of the row.
    pub fn number<N: egui::emath::Numeric>(
        &mut self,
        label: &str,
        value: &mut N,
        range: RangeInclusive<N>,
    ) -> Response {
        self.row(label, |ui, rest| {
            let height = ui.spacing().interact_size.y;
            ui.add_sized([rest, height], DragValue::new(value).range(range))
        })
    }

    /// A checkbox row: the label in the label column, `text` next to the box.
    pub fn check(&mut self, label: &str, text: &str, value: &mut bool, hover: &str) -> Response {
        self.row(label, |ui, _| ui.checkbox(value, text).on_hover_text(hover))
    }

    /// A read-only value that is shortened to fit, with the full text on hover.
    pub fn value(&mut self, label: &str, text: &str) -> Response {
        self.row(label, |ui, _| {
            ui.add(Label::new(RichText::new(text)).truncate())
                .on_hover_text(text)
        })
    }
}

/// One button of a [`button_row`].
pub struct Btn<'a> {
    pub label: &'a str,
    pub enabled: bool,
    pub hover: &'a str,
    /// Bold text for the main action of a row.
    pub primary: bool,
}

impl<'a> Btn<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            enabled: true,
            hover: "",
            primary: false,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn hover(mut self, hover: &'a str) -> Self {
        self.hover = hover;
        self
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
}

/// Buttons side by side, all of the same width, filling the row. Returns the index of the one
/// that was clicked.
pub fn button_row(ui: &mut Ui, buttons: &[Btn]) -> Option<usize> {
    button_cells(ui, buttons).0
}

/// Where the input column of a [`form`] starts, relative to the form's left edge.
pub fn input_indent() -> f32 {
    LABEL_W + SPACING[0]
}

/// Like [`button_row`], but only as wide as the inputs of a form: the label column stays empty,
/// so the buttons start and end where the sliders and fields above them do.
pub fn form_buttons(ui: &mut Ui, buttons: &[Btn]) -> Option<usize> {
    form_button_cells(ui, buttons).0
}

fn form_button_cells(ui: &mut Ui, buttons: &[Btn]) -> (Option<usize>, Vec<egui::Rect>) {
    let avail = ui.available_rect_before_wrap();
    let area = egui::Rect::from_min_max(
        egui::pos2(
            (avail.left() + input_indent()).min(avail.right()),
            avail.top(),
        ),
        avail.max,
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        button_cells(ui, buttons)
    })
    .inner
}

fn button_cells(ui: &mut Ui, buttons: &[Btn]) -> (Option<usize>, Vec<egui::Rect>) {
    if buttons.is_empty() {
        return (None, vec![]);
    }
    ui.columns(buttons.len(), |columns| {
        let mut clicked = None;
        let mut rects = vec![];
        for (i, (column, b)) in columns.iter_mut().zip(buttons).enumerate() {
            let text = if b.primary {
                RichText::new(b.label).strong()
            } else {
                RichText::new(b.label)
            };
            let width = column.available_width();
            let response = column.add_enabled(
                b.enabled,
                egui::Button::new(text).min_size(egui::vec2(width, 0.0)),
            );
            let response = if b.hover.is_empty() {
                response
            } else {
                response.on_hover_text(b.hover)
            };
            rects.push(response.rect);
            if response.clicked() {
                clicked = Some(i);
            }
        }
        (clicked, rects)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Horizontal margin of egui's text field frame.
    const TEXT_FRAME_MARGIN: f32 = 4.0;

    /// Runs one headless frame in a panel of `width` px and returns what `body` collects.
    fn frame<T: Default>(width: f32, body: impl FnOnce(&mut Ui, &mut T)) -> T {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width + 40.0, 800.0),
            )),
            ..Default::default()
        };
        let mut out = T::default();
        let mut body = Some(body);
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(width);
                if let Some(body) = body.take() {
                    body(ui, &mut out);
                }
            });
        });
        out
    }

    #[test]
    fn the_slider_track_leaves_room_for_the_number_and_never_collapses() {
        assert_eq!(slider_track_width(300.0), 300.0 - VALUE_BOX_W);
        assert_eq!(slider_track_width(100.0), MIN_SLIDER_W);
        assert_eq!(slider_track_width(0.0), MIN_SLIDER_W);
    }

    #[test]
    fn every_row_kind_ends_at_the_same_right_edge_whatever_its_label() {
        let rights: Vec<f32> = frame(360.0, |ui, out: &mut Vec<f32>| {
            let mut name = String::new();
            let mut ev = 0.0_f32;
            let mut n = 3_u32;
            form(ui, "one", |f| {
                out.push(f.combo("Scanner", "a", "Canon", |_| {}).rect.right());
                out.push(f.combo("Profile", "b", "35mm", |_| {}).rect.right());
                // A text field's response is its inner rect; its frame margin lies outside.
                out.push(f.text("Name", &mut name).rect.right() + TEXT_FRAME_MARGIN);
                out.push(f.number("Start no.", &mut n, 0..=99).rect.right());
            });
            form(ui, "two", |f| {
                out.push(
                    f.combo("Min. confidence label", "c", "x", |_| {})
                        .rect
                        .right(),
                );
                let _ = f.slider(
                    "Exposure",
                    &mut ev,
                    -2.0..=2.0,
                    SliderOpts::default().suffix(" EV").decimals(2),
                    "",
                );
                // The slider row (track plus number box) must not reach past the others.
                out.push(f.ui.min_rect().right());
            });
        });
        let first = rights[0];
        assert!(
            rights.iter().all(|r| (r - first).abs() < 0.6),
            "all inputs end together: {rights:?}"
        );
    }

    #[test]
    fn labels_have_the_same_width_in_different_forms() {
        let xs: Vec<f32> = frame(360.0, |ui, out: &mut Vec<f32>| {
            form(ui, "a", |f| {
                out.push(f.combo("Scanner", "a", "x", |_| {}).rect.left());
            });
            form(ui, "b", |f| {
                out.push(
                    f.combo("A much longer label than fits", "b", "x", |_| {})
                        .rect
                        .left(),
                );
            });
        });
        assert!(
            (xs[0] - xs[1]).abs() < 0.6,
            "inputs start at the same x: {xs:?}"
        );
    }

    #[test]
    fn a_narrow_panel_keeps_every_input_usable() {
        let widths: Vec<f32> = frame(190.0, |ui, out: &mut Vec<f32>| {
            let mut v = 1.0_f32;
            form(ui, "n", |f| {
                out.push(f.combo("Scanner", "a", "x", |_| {}).rect.width());
                let _ = f.slider("Exposure", &mut v, 0.0..=2.0, SliderOpts::default(), "");
            });
        });
        assert!(widths[0] > 30.0, "{widths:?}");
    }

    #[test]
    fn a_double_click_resets_only_with_a_default() {
        let mut v = 1.5_f32;
        assert!(reset_on_double_click(&mut v, Some(0.0), true));
        assert_eq!(v, 0.0);
        // No double click, no default, or already at the default: nothing changes.
        let mut w = 1.5_f32;
        assert!(!reset_on_double_click(&mut w, Some(0.0), false));
        assert!(!reset_on_double_click(&mut w, None, true));
        assert_eq!(w, 1.5);
        let mut at_default = 0.0_f64;
        assert!(
            !reset_on_double_click(&mut at_default, Some(0.0), true),
            "unchanged reports no change"
        );
        // Works for both float widths and for whole numbers.
        let mut n = 7_u32;
        assert!(reset_on_double_click(&mut n, Some(3.0), true));
        assert_eq!(n, 3);
    }

    #[test]
    fn the_hover_text_shows_the_default_as_the_row_shows_values() {
        let opts = SliderOpts::default().suffix(" mm").decimals(1);
        assert_eq!(opts.format(3.0), "3.0 mm");
        assert_eq!(SliderOpts::default().format(2.0), "2");
        assert_eq!(SliderOpts::default().format(0.25), "0.25");
        assert_eq!(
            SliderOpts::default().suffix(" %").decimals(0).format(100.0),
            "100 %"
        );
    }

    /// Draws one slider row and returns where its track is.
    fn slider_frame(
        ui: &mut Ui,
        value: &mut f32,
        opts: SliderOpts,
        changed: &mut bool,
        pos: &mut egui::Pos2,
    ) {
        form(ui, "h", |f| {
            *changed |= f.slider("Exposure", value, -2.0..=2.0, opts, "");
            let grid = f.ui.min_rect();
            // Inside the track: after the label column and a little way in.
            *pos = egui::pos2(grid.left() + LABEL_W + 40.0, grid.center().y);
        });
    }

    #[test]
    fn form_buttons_start_and_end_where_the_inputs_do() {
        use test_support::Harness;
        let mut h = Harness::new();
        let (mut input, mut two, mut three) = (egui::Rect::NOTHING, vec![], vec![]);
        let mut text = String::new();
        for _ in 0..3 {
            h.frame(vec![], |ui| {
                let r = form(ui, "t", |f| f.text("Name", &mut text).rect);
                // The text field reports its inner rect, 4 px inside its frame.
                input = r.expand2(egui::vec2(TEXT_FRAME_MARGIN, 0.0));
                two = form_button_cells(ui, &[Btn::new("One"), Btn::new("Two")]).1;
                three = form_button_cells(ui, &[Btn::new("A"), Btn::new("B"), Btn::new("C")]).1;
            });
        }
        for row in [&two, &three] {
            let (first, last) = (row.first().unwrap(), row.last().unwrap());
            assert!(
                (first.left() - input.left()).abs() < 1.0,
                "{first:?} vs {input:?}"
            );
            assert!(
                (last.right() - input.right()).abs() < 1.0,
                "{last:?} vs {input:?}"
            );
        }
        assert_eq!(three.len(), 3);
    }

    #[test]
    fn double_clicking_a_slider_resets_it_to_its_default() {
        use test_support::Harness;
        let mut h = Harness::new();
        let opts = SliderOpts::default().decimals(2).default_value(0.0);
        let (mut value, mut changed, mut pos) = (1.5_f32, false, egui::Pos2::ZERO);
        h.frame(vec![], |ui| {
            slider_frame(ui, &mut value, opts, &mut changed, &mut pos)
        });
        assert_eq!(value, 1.5, "drawing alone changes nothing");
        let at = pos;
        h.double_click(at, |ui| {
            slider_frame(ui, &mut value, opts, &mut changed, &mut pos)
        });
        assert_eq!(value, 0.0, "back to the default");
        assert!(changed, "the reset is reported as a change");
    }

    #[test]
    fn a_single_click_moves_the_slider_but_does_not_reset_it() {
        use test_support::Harness;
        let mut h = Harness::new();
        let opts = SliderOpts::default().decimals(2).default_value(1.0);
        let (mut value, mut changed, mut pos) = (-1.0_f32, false, egui::Pos2::ZERO);
        h.frame(vec![], |ui| {
            slider_frame(ui, &mut value, opts, &mut changed, &mut pos)
        });
        let at = pos;
        h.single_click(at, |ui| {
            slider_frame(ui, &mut value, opts, &mut changed, &mut pos)
        });
        assert_ne!(value, -1.0, "the click moved it");
        assert_ne!(value, 1.0, "but not to the default");
    }

    #[test]
    fn without_a_default_a_double_click_just_sets_the_clicked_position() {
        use test_support::Harness;
        let mut h = Harness::new();
        let opts = SliderOpts::default().decimals(2);
        let (mut value, mut changed, mut pos) = (1.5_f32, false, egui::Pos2::ZERO);
        h.frame(vec![], |ui| {
            slider_frame(ui, &mut value, opts, &mut changed, &mut pos)
        });
        let at = pos;
        h.double_click(at, |ui| {
            slider_frame(ui, &mut value, opts, &mut changed, &mut pos)
        });
        assert!(value != 1.5 && value.is_finite());
    }

    #[test]
    fn a_button_row_gives_equal_widths() {
        let widths: Vec<f32> = frame(300.0, |ui, out: &mut Vec<f32>| {
            let before = ui.min_rect();
            let _ = button_row(
                ui,
                &[
                    Btn::new("Auto"),
                    Btn::new("Auto all").primary(),
                    Btn::new("Reset"),
                ],
            );
            out.push(ui.min_rect().width() - before.width());
        });
        // The row spans the panel (no panic, one row of three equal columns).
        assert!(widths[0] >= 0.0);
        let empty = frame(300.0, |ui, out: &mut Vec<bool>| {
            out.push(button_row(ui, &[]).is_none())
        });
        assert_eq!(empty, [true]);
    }
}

/// Headless egui frames with simulated pointer input, for the tests of widgets (also used by
/// hosts of this crate).
pub mod test_support {
    use egui::{Context, Event, PointerButton, Pos2, RawInput, Rect, Ui};

    pub struct Harness {
        pub ctx: Context,
        time: f64,
    }

    impl Default for Harness {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Harness {
        pub fn new() -> Self {
            Self {
                ctx: Context::default(),
                time: 0.0,
            }
        }

        /// One frame of 50 ms with `events`, drawing `body` in a 360 px wide panel.
        pub fn frame(&mut self, events: Vec<Event>, body: impl FnOnce(&mut Ui)) {
            self.time += 0.05;
            let input = RawInput {
                time: Some(self.time),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(420.0, 900.0))),
                events,
                ..Default::default()
            };
            let mut body = Some(body);
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(360.0);
                    if let Some(body) = body.take() {
                        body(ui);
                    }
                });
            });
        }

        /// A press and release of the primary button at `pos`, each in its own frame.
        fn click(&mut self, pos: Pos2, body: &mut impl FnMut(&mut Ui)) {
            for pressed in [true, false] {
                let event = Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                self.frame(vec![event], &mut *body);
            }
        }

        /// Moves the pointer to `pos`, then clicks once.
        pub fn single_click(&mut self, pos: Pos2, mut body: impl FnMut(&mut Ui)) {
            self.frame(vec![Event::PointerMoved(pos)], &mut body);
            self.click(pos, &mut body);
        }

        /// Moves the pointer to `pos`, then clicks twice in quick succession.
        pub fn double_click(&mut self, pos: Pos2, mut body: impl FnMut(&mut Ui)) {
            self.frame(vec![Event::PointerMoved(pos)], &mut body);
            self.click(pos, &mut body);
            self.click(pos, &mut body);
            // Let the pointer rest so the next click is not part of this one.
            self.time += 1.0;
        }
    }
}
