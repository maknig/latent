//! Edit mode of the image view: sliders, crop and dust on a 16-bit preview of the file.
//!
//! Edits only change the preview until **Apply** writes them to the file at full resolution
//! (see [`latent_image_edit_ui::editor`]). The original is backed up to `.originals/` first.
//! Rotate is not part of the edits: it rewrites the file at once, outside this mode.

use eframe::egui;
use latent_image_edit::RawImage;
use latent_image_edit_ui::editor::{self, EditorState, Edits, FULL};
use latent_image_edit_ui::form::{form, form_buttons, Btn};
use latent_image_edit_ui::presets::{PresetUi, Presets};
use latent_image_edit_ui::sliders::{adjust_groups, preset_row};
use latent_image_edit_ui::{histogram, ACCENT};
use std::path::Path;
use std::sync::mpsc::Receiver;

/// Brush radius range of the dust tool, as a fraction of the picture's width.
pub const BRUSH: std::ops::RangeInclusive<f32> = 0.002..=0.03;
pub const SIDE_W: f32 = 300.0;
const HISTOGRAM_H: f32 = 100.0;

/// Which of the markers (darkest, brightest and most neutral spot) are shown on the picture.
#[derive(Default, Clone, Copy)]
pub struct Marks {
    pub black: bool,
    pub white: bool,
    pub gray: bool,
}

/// The Black / White / Gray check boxes, the same row in View and Edit mode. Gray needs a colour
/// picture.
pub fn marks_row(ui: &mut egui::Ui, marks: &mut Marks, colour: bool) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut marks.black, "Black");
        ui.checkbox(&mut marks.white, "White");
        ui.add_enabled(colour, egui::Checkbox::new(&mut marks.gray, "Gray"))
            .on_hover_text("The most neutral spot of the picture");
    });
}

type Loaded = Result<(RawImage, [u32; 2]), String>;

/// Where the user's slider presets live: `<config dir>/latent/presets.toml`.
pub fn load_presets() -> PresetUi {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")));
    let (presets, error) = match dir {
        Some(d) => Presets::load_from(&d.join("latent").join("presets.toml")),
        None => (Presets::default(), None),
    };
    let mut ui = PresetUi::new(presets);
    ui.error = error;
    ui
}

/// One image open in edit mode.
pub struct Session {
    /// Archive-relative path of the image.
    pub rel: String,
    pub state: EditorState,
    /// Dust mode: dragging over the picture marks spots.
    pub dust: bool,
    /// Gray tool: the next click on the picture picks a neutral spot.
    pub gray_tool: bool,
    /// The spot the colour balance was last taken from (fractions of the picture).
    pub gray: Option<(f32, f32)>,
    load: Option<Receiver<Loaded>>,
    apply: Option<Receiver<Result<RawImage, String>>>,
}

impl Session {
    pub fn new(ctx: &egui::Context, root: &Path, rel: &str) -> Self {
        let state = EditorState::new(root, rel);
        let load = Some(editor::load_async(ctx, state.path.clone()));
        Self {
            rel: rel.to_string(),
            state,
            dust: false,
            gray_tool: false,
            gray: None,
            load,
            apply: None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.state.dirty()
    }

    /// The file changed on disk (rotated, restored or applied): decode it again. `keep` carries
    /// the slider edits over; a crop or dust spots drawn on the old picture are dropped.
    pub fn reload(&mut self, ctx: &egui::Context, root: &Path, keep: bool) {
        let mut edits = std::mem::take(&mut self.state.edits);
        edits.crop = None;
        edits.spots.clear();
        self.state.reset_for_reload(root);
        if keep {
            self.state.edits = edits;
        }
        self.dust = false;
        self.gray_tool = false;
        self.gray = None;
        self.load = Some(editor::load_async(ctx, self.state.path.clone()));
    }

    /// Takes in the results of the worker threads and shows the finished previews. Returns
    /// `Some` when an Apply finished.
    pub fn poll(&mut self, ctx: &egui::Context, root: &Path) -> Option<Result<(), String>> {
        if let Some(rx) = &self.load {
            if let Ok(result) = rx.try_recv() {
                self.load = None;
                match result {
                    Ok((preview, size)) => self.state.loaded(preview, size),
                    Err(e) => {
                        self.state.busy = None;
                        self.state.error = Some(e);
                    }
                }
                self.state.has_original =
                    latent_image_edit_ui::edit::has_original(root, &self.state.path);
            }
        }
        let done = self.apply.as_ref().and_then(|rx| rx.try_recv().ok());
        let result = done.map(|r| r.map(|_| ()));
        if let Some(r) = &result {
            self.apply = None;
            self.state.busy = None;
            if r.is_ok() {
                self.reload(ctx, root, false);
            } else {
                self.state.has_original =
                    latent_image_edit_ui::edit::has_original(root, &self.state.path);
            }
        }
        result
    }

    /// Writes the edits to the file on a worker thread.
    pub fn apply(&mut self, ctx: &egui::Context, root: &Path) {
        if self.state.busy.is_some() || !self.state.editable || !self.dirty() {
            return;
        }
        self.state.busy = Some("Applying");
        self.apply = Some(editor::apply_async(
            ctx,
            root.to_path_buf(),
            self.state.path.clone(),
            self.state.edits.clone(),
        ));
    }

    /// Throws the pending edits away.
    pub fn revert(&mut self) {
        self.state.edits = Edits::default();
        self.dust = false;
        self.gray_tool = false;
        self.gray = None;
    }

    fn colour(&self) -> bool {
        self.state.base.as_ref().is_some_and(|b| b.channels == 3)
    }

    /// Takes warmth and tint from the neutral spot at `(u, v)` (fractions of the picture).
    pub fn pick_gray(&mut self, u: f32, v: f32) {
        self.gray_tool = false;
        let Some(base) = &self.state.base else { return };
        if let Some((warmth, tint)) = editor::neutral_gray(base, &self.state.edits, u, v) {
            self.state.edits.adjust.warmth = warmth;
            self.state.edits.adjust.tint = tint;
            self.gray = Some((u, v));
        }
    }

    /// Shows what the render thread finished and asks for the next preview. Call before and
    /// after the window is drawn.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        self.state.refresh(ctx);
    }

    fn can_edit(&self) -> bool {
        self.state.editable && self.state.busy.is_none()
    }
}

/// The edit tools of the toolbar: Crop, Dust (with brush size, Undo and Clear), Apply, Revert.
/// `brush` is the dust radius. Returns true when Apply was pressed.
pub fn tools(ui: &mut egui::Ui, s: &mut Session, brush: &mut f32) -> bool {
    let can = s.can_edit();
    let cropping = s.state.edits.crop.is_some();
    if ui
        .add_enabled(can, egui::SelectableLabel::new(cropping, "Crop"))
        .on_hover_text("Drag on the image to cut off borders; Apply writes the crop")
        .clicked()
    {
        s.state.edits.crop = if cropping { None } else { Some(FULL) };
        s.dust = false;
        s.gray_tool = false;
    }
    if ui
        .add_enabled(can, egui::SelectableLabel::new(s.dust, "Dust"))
        .on_hover_text("Click or drag over dust and scratches to mark them; Apply heals them")
        .clicked()
    {
        s.dust = !s.dust;
        if s.dust {
            s.state.edits.crop = None;
            s.gray_tool = false;
        }
    }
    let colour = s.colour();
    if ui
        .add_enabled(
            can && colour,
            egui::SelectableLabel::new(s.gray_tool, "Gray"),
        )
        .on_hover_text("Click a neutral gray area of the picture to remove the colour cast")
        .clicked()
    {
        s.gray_tool = !s.gray_tool;
        if s.gray_tool {
            s.state.edits.crop = None;
            s.dust = false;
        }
    }
    let marker = s.state.markers.and_then(|m| m.gray);
    if ui
        .add_enabled(
            can && colour && marker.is_some(),
            egui::Button::new("Gray from marker"),
        )
        .on_hover_text("Remove the colour cast at the Gray marker")
        .clicked()
    {
        if let Some((u, v)) = marker {
            s.pick_gray(u, v);
        }
    }
    if s.dust {
        ui.label("Size");
        ui.add(egui::Slider::new(brush, BRUSH).show_value(false));
        let marked = !s.state.edits.spots.is_empty();
        if ui.add_enabled(marked, egui::Button::new("Undo")).clicked() {
            s.state.edits.spots.pop();
        }
        if ui
            .add_enabled(
                marked,
                egui::Button::new(format!("Clear ({})", s.state.edits.spots.len())),
            )
            .clicked()
        {
            s.state.edits.spots.clear();
        }
    }
    ui.separator();
    let apply = ui
        .add_enabled(
            can && s.dirty(),
            egui::Button::new(egui::RichText::new("Apply").strong()).fill(ACCENT),
        )
        .on_hover_text("Write the edits to the file (the original is backed up first)")
        .clicked();
    if ui
        .add_enabled(s.dirty(), egui::Button::new("Revert"))
        .on_hover_text("Throw the unsaved edits away")
        .clicked()
    {
        s.revert();
    }
    apply
}

/// The side column of edit mode: histogram, state line, film, sliders. `scroll_h` is the
/// height of the scrolling slider area.
pub fn side_column(
    ui: &mut egui::Ui,
    s: &mut Session,
    presets: &mut PresetUi,
    marks: &mut Marks,
    scroll_h: f32,
) {
    histogram::paint(ui, &s.state.histogram, egui::vec2(SIDE_W, HISTOGRAM_H));
    let colour = s.colour();
    marks_row(ui, marks, colour);
    ui.add_space(4.0);
    let st = &s.state;
    let mut line = vec![];
    if let Some(busy) = st.busy {
        line.push(format!("{busy}…"));
    } else if st.dirty() {
        line.push("unsaved edits".to_string());
    }
    if st.file_size[0] > 0 {
        line.push(format!("{} × {}", st.file_size[0], st.file_size[1]));
    }
    if st.rendering() {
        line.push("rendering…".to_string());
    }
    if !st.edits.spots.is_empty() {
        line.push(format!("{} dust spots", st.edits.spots.len()));
    }
    if st.has_original {
        line.push("backup exists".into());
    }
    if !st.editable {
        line.push("view only (TIFF, PNG and JPEG can be edited)".into());
    }
    ui.weak(line.join(" · "));
    if let Some(e) = &st.error {
        ui.colored_label(egui::Color32::LIGHT_RED, e);
    }
    ui.add_space(4.0);
    let enabled = s.can_edit();
    egui::ScrollArea::vertical()
        .id_salt("edit_sliders")
        .max_height(scroll_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(SIDE_W - 12.0);
            let reset = ui
                .add_enabled_ui(enabled, |ui| sliders(ui, &mut s.state, presets))
                .inner;
            if reset {
                s.gray = None;
            }
        });
}

/// Returns true when "Reset sliders" was pressed.
fn sliders(ui: &mut egui::Ui, st: &mut EditorState, presets: &mut PresetUi) -> bool {
    let colour = st.base.as_ref().is_none_or(|b| b.channels == 3);
    form(ui, "edit-film", |f| {
        f.check(
            "Film",
            "negative",
            &mut st.edits.negative,
            "Invert and remove the orange mask",
        );
    });
    if let Some(p) = preset_row(
        ui,
        "edit-preset",
        presets,
        (
            &st.edits.adjust,
            Some((&st.edits.clarity, &st.edits.sharpen)),
        ),
    ) {
        st.edits.adjust = p.adjust();
        st.edits.clarity = p.clarity();
        st.edits.sharpen = p.sharpen();
    }
    ui.add_space(4.0);
    let edits = &mut st.edits;
    adjust_groups(
        ui,
        "edit",
        &mut edits.adjust,
        Some((&mut edits.clarity, &mut edits.sharpen)),
        colour,
    );
    let already_8 = st.source_depth == latent_image_edit::BitDepth::Eight;
    let mut eight = st.edits.eight_bit || already_8;
    ui.add_enabled_ui(!already_8, |ui| {
        form(ui, "edit-depth", |f| {
            f.check(
                "Output",
                "Save as 8 bit",
                &mut eight,
                if already_8 {
                    "This file is already 8 bit"
                } else {
                    "Halves the file when applied; Restore original brings the 16 bit back"
                },
            );
        });
    });
    if !already_8 {
        st.edits.eight_bit = eight;
    }
    ui.add_space(4.0);
    let buttons = [
        Btn::new("Auto")
            .enabled(st.base.is_some())
            .hover("Black and white point from the picture (or the crop)"),
        Btn::new("Reset sliders").hover("Film, sliders and sharpening back to neutral"),
    ];
    match form_buttons(ui, &buttons) {
        Some(0) => {
            if let Some(base) = &st.base {
                let a = editor::auto_levels(base, &st.edits);
                st.edits.adjust.black = a.black;
                st.edits.adjust.white = a.white;
            }
        }
        Some(_) => {
            st.edits.negative = false;
            st.edits.adjust = Default::default();
            st.edits.clarity = Default::default();
            st.edits.sharpen = Default::default();
            return true;
        }
        None => {}
    }
    false
}
