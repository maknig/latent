//! Generic egui front end: table view and edit form generated from the schema.
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use fa_data::{default_value, format_value, parse_value, Store, TableData, Value};
use fa_engine::{Engine, Query};
use fa_schema::{Field, FieldType};
#[cfg(not(target_arch = "wasm32"))]
use fa_schema::TableRole;

#[cfg(not(target_arch = "wasm32"))]
mod histogram;
mod print;
#[cfg(not(target_arch = "wasm32"))]
mod images;
#[cfg(not(target_arch = "wasm32"))]
mod people;
#[cfg(not(target_arch = "wasm32"))]
mod similar;
#[cfg(not(target_arch = "wasm32"))]
mod tagger;

/// Decoded preview texture plus its histogram and black/white points.
#[cfg(not(target_arch = "wasm32"))]
type Shown = (egui::TextureHandle, Option<(histogram::Histogram, (f32, f32), (f32, f32))>);

/// Open image view: which film row and image, plus the text being edited.
#[cfg(not(target_arch = "wasm32"))]
struct Preview {
    table: usize,
    row: u64,
    index: usize,
    /// Archive-relative path whose info is in `tags` / `location`.
    loaded: Option<String>,
    tags: String,
    location: String,
    /// 1.0 fits the image to the view.
    zoom: f32,
    black: bool,
    white: bool,
    /// Last decoded image with its histogram and black/white points; shown while the next one loads.
    shown: Option<Shown>,
    /// Direction of the last Back/Next (-1 or 1); the image that way is preloaded.
    dir: i32,
    /// Height of the side column in the previous frame; the image viewport follows it.
    side_h: f32,
    /// Crop mode: `[left, top, right, bottom]` as fractions of the image.
    crop: Option<[f32; 4]>,
    grab: Option<Grab>,
}

/// What a crop drag moves: a new rectangle from an anchor, the edges flagged `[left, top, right, bottom]`, or the whole rectangle.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
enum Grab {
    New(egui::Pos2),
    Edges([bool; 4]),
    Move,
}

#[cfg(not(target_arch = "wasm32"))]
impl Preview {
    fn new(table: usize, row: u64, index: usize) -> Self {
        Preview { table, row, index, loaded: None, tags: String::new(), location: String::new(), zoom: 1.0, black: false, white: false, shown: None, dir: 1, side_h: 0.0, crop: None, grab: None }
    }
}

/// Handles a crop drag on the image at `rect` and paints the dimmed outside, outline and thirds.
#[cfg(not(target_arch = "wasm32"))]
fn crop_overlay(ui: &egui::Ui, resp: &egui::Response, painter: &egui::Painter, rect: egui::Rect, crop: &mut [f32; 4], grab: &mut Option<Grab>) {
    const HIT: f32 = 10.0;
    const MIN: f32 = 0.02;
    let to_norm = |p: egui::Pos2| egui::pos2(((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0), ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0));
    let screen = |c: &[f32; 4]| egui::Rect::from_min_max(rect.min + egui::vec2(c[0] * rect.width(), c[1] * rect.height()), rect.min + egui::vec2(c[2] * rect.width(), c[3] * rect.height()));
    let edges_at = |p: egui::Pos2, r: egui::Rect| -> Option<[bool; 4]> {
        if !r.expand(HIT).contains(p) {
            return None;
        }
        let e = [(p.x - r.left()).abs() < HIT, (p.y - r.top()).abs() < HIT, (p.x - r.right()).abs() < HIT, (p.y - r.bottom()).abs() < HIT];
        // Only the nearer of two opposite edges when the rectangle is tiny.
        let e = [e[0], e[1], e[2] && !e[0], e[3] && !e[1]];
        e.iter().any(|&b| b).then_some(e)
    };
    let r = screen(crop);
    if let (Some(p), None) = (resp.hover_pos(), *grab) {
        ui.ctx().set_cursor_icon(match edges_at(p, r) {
            Some([l, t, rt, b]) if (l || rt) && (t || b) => if l == t { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw },
            Some([l, _, rt, _]) if l || rt => egui::CursorIcon::ResizeHorizontal,
            Some(_) => egui::CursorIcon::ResizeVertical,
            None if r.contains(p) => egui::CursorIcon::Move,
            None => egui::CursorIcon::Crosshair,
        });
    }
    if resp.drag_started() {
        *grab = resp.interact_pointer_pos().map(|p| match edges_at(p, r) {
            Some(e) => Grab::Edges(e),
            None if r.contains(p) => Grab::Move,
            None => Grab::New(to_norm(p)),
        });
    }
    if resp.dragged() {
        if let (Some(g), Some(p)) = (*grab, resp.interact_pointer_pos()) {
            match g {
                Grab::New(a) => {
                    let b = to_norm(p);
                    *crop = [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)];
                }
                Grab::Edges(e) => {
                    let n = to_norm(p);
                    for (i, &on) in e.iter().enumerate() {
                        if on {
                            crop[i] = if i % 2 == 0 { n.x } else { n.y };
                        }
                    }
                    // Keep a minimum size and the right order by pushing the dragged edge back.
                    if e[0] {
                        crop[0] = crop[0].min(crop[2] - MIN);
                    }
                    if e[2] {
                        crop[2] = crop[2].max(crop[0] + MIN);
                    }
                    if e[1] {
                        crop[1] = crop[1].min(crop[3] - MIN);
                    }
                    if e[3] {
                        crop[3] = crop[3].max(crop[1] + MIN);
                    }
                }
                Grab::Move => {
                    let d = resp.drag_delta() / rect.size();
                    let dx = d.x.clamp(-crop[0], 1.0 - crop[2]);
                    let dy = d.y.clamp(-crop[1], 1.0 - crop[3]);
                    *crop = [crop[0] + dx, crop[1] + dy, crop[2] + dx, crop[3] + dy];
                }
            }
        }
    }
    if resp.drag_stopped() {
        *grab = None;
    }
    let r = screen(crop);
    let dim = egui::Color32::from_black_alpha(150);
    painter.rect_filled(egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, r.top())), 0.0, dim);
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(rect.min.x, r.bottom()), rect.max), 0.0, dim);
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(rect.min.x, r.top()), egui::pos2(r.left(), r.bottom())), 0.0, dim);
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(r.right(), r.top()), egui::pos2(rect.max.x, r.bottom())), 0.0, dim);
    let thin = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(70));
    for i in 1..3 {
        let f = i as f32 / 3.0;
        painter.line_segment([egui::pos2(r.left() + f * r.width(), r.top()), egui::pos2(r.left() + f * r.width(), r.bottom())], thin);
        painter.line_segment([egui::pos2(r.left(), r.top() + f * r.height()), egui::pos2(r.right(), r.top() + f * r.height())], thin);
    }
    painter.rect_stroke(r, 0.0, egui::Stroke::new(2.0_f32, egui::Color32::WHITE), egui::StrokeKind::Middle);
    for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom(), r.center_top(), r.center_bottom(), r.left_center(), r.right_center()] {
        painter.rect_filled(egui::Rect::from_center_size(c, egui::vec2(8.0, 8.0)), 1.0, ACCENT);
    }
}

const ACCENT: egui::Color32 = egui::Color32::from_rgb(70, 130, 200);

/// Marker: ring at `pos` (0..1 within `rect`) with a pill label beside it, on the side facing the image centre.
/// `white` mirrors the colours: white ring for the black point, dark ring for the white point.
#[cfg(not(target_arch = "wasm32"))]
fn annotate(painter: &egui::Painter, rect: egui::Rect, pos: (f32, f32), label: &str, white: bool) {
    const RING: f32 = 8.0;
    const GAP: f32 = 6.0;
    let ring = if white { egui::Color32::from_gray(20) } else { egui::Color32::WHITE };
    let halo = if white { egui::Color32::from_white_alpha(150) } else { egui::Color32::from_black_alpha(150) };
    let p = rect.min + egui::vec2(pos.0 * rect.width(), pos.1 * rect.height());
    painter.circle_stroke(p, RING, egui::Stroke::new(4.0_f32, halo));
    painter.circle_stroke(p, RING, egui::Stroke::new(2.0_f32, ring));
    let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(13.0), egui::Color32::WHITE);
    let size = galley.size() + egui::vec2(26.0, 8.0);
    let x = if pos.0 < 0.5 { p.x + RING + GAP } else { p.x - RING - GAP - size.x };
    let inner = rect.shrink(4.0);
    let y = (p.y - size.y / 2.0).clamp(inner.top(), (inner.bottom() - size.y).max(inner.top()));
    let pill = egui::Rect::from_min_size(egui::pos2(x, y), size);
    painter.rect_filled(pill, 4.0, egui::Color32::from_black_alpha(200));
    painter.rect_stroke(pill, 4.0, egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(60)), egui::StrokeKind::Inside);
    let dot_pos = pill.left_center() + egui::vec2(10.0, 0.0);
    painter.circle_filled(dot_pos, 3.5, if white { egui::Color32::WHITE } else { egui::Color32::BLACK });
    if !white {
        painter.circle_stroke(dot_pos, 3.5, egui::Stroke::new(1.0_f32, egui::Color32::WHITE));
    }
    painter.galley(pill.left_top() + egui::vec2(18.0, 4.0), galley, egui::Color32::WHITE);
}


/// Small dark pill with `text`, its top-left (or top-right if `right`) corner at `corner`, inset by 4px.
fn overlay_pill(painter: &egui::Painter, corner: egui::Pos2, right: bool, text: &str) {
    let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE);
    let size = galley.size() + egui::vec2(16.0, 6.0);
    let x = if right { corner.x - 4.0 - size.x } else { corner.x + 4.0 };
    let pill = egui::Rect::from_min_size(egui::pos2(x, corner.y + 4.0), size);
    painter.rect_filled(pill, 4.0, egui::Color32::from_black_alpha(200));
    painter.rect_stroke(pill, 4.0, egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(60)), egui::StrokeKind::Inside);
    painter.galley(pill.left_top() + egui::vec2(8.0, 3.0), galley, egui::Color32::WHITE);
}

enum Msg {
    Csv(String),
    Open(String, String),
    Error(String),
    #[cfg(not(target_arch = "wasm32"))]
    AddImages(Vec<std::path::PathBuf>),
    /// Copied images: (table index, row id, archive-relative paths, errors).
    #[cfg(not(target_arch = "wasm32"))]
    Copied(usize, u64, Vec<String>, Vec<String>),
}

#[cfg(target_arch = "wasm32")]
fn spawn(fut: impl std::future::Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(fut);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    std::thread::spawn(move || pollster::block_on(fut));
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = egui::Color32::from_gray(30);
    v.window_fill = egui::Color32::from_gray(36);
    v.extreme_bg_color = egui::Color32::from_gray(22);
    v.faint_bg_color = egui::Color32::from_gray(36);
    v.selection.bg_fill = ACCENT;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(48));
    v.widgets.hovered.bg_fill = egui::Color32::from_gray(55);
    v.striped = true;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}

/// Form editor for an `image_ref`: thumbnail, path and buttons. Returns true when "Choose..." was clicked.
#[cfg(not(target_arch = "wasm32"))]
fn image_ref_editor(ui: &mut egui::Ui, ctx: &egui::Context, thumbs: &mut images::Thumbs, base_dir: &std::path::Path, text: &mut String) -> bool {
    let mut choose = false;
    if !text.trim().is_empty() {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 72.0), egui::Sense::hover());
        thumbs.paint(ui, ctx, &base_dir.join(text.trim()), rect, 320);
    }
    ui.vertical(|ui| {
        ui.label(if text.trim().is_empty() { "(none)" } else { text.trim() });
        ui.horizontal(|ui| {
            choose = ui.button("Choose...").clicked();
            if ui.add_enabled(!text.is_empty(), egui::Button::new("Clear")).clicked() {
                text.clear();
            }
        });
    });
    choose
}

type SaveHook =Box<dyn FnMut(&Store) -> Result<(), String>>;

struct Form {
    id: Option<u64>,
    texts: Vec<String>,
    errors: Vec<String>,
}

/// Dialog for choosing the film image an enlargement comes from.
#[cfg(not(target_arch = "wasm32"))]
struct Picker {
    table: usize,
    row: Option<u64>,
}

pub struct App {
    pub engine: Engine,
    table: usize,
    query: Query,
    selected: Option<u64>,
    form: Option<Form>,
    pub status: String,
    /// Native save hook: called with the store when the user saves.
    pub on_save: Option<SaveHook>,
    /// Save to eframe storage (localStorage on web) for autosave.
    pub persist: bool,
    styled: bool,
    col_widths: std::collections::HashMap<(usize, usize), f32>,
    show_filters: bool,
    /// Open print dialog.
    print: Option<print::PrintOpts>,
    /// Pending "delete unused images" confirmation.
    #[cfg(not(target_arch = "wasm32"))]
    unused: Option<Vec<std::path::PathBuf>>,
    /// Directory image paths are relative to (the data file's folder).
    pub base_dir: std::path::PathBuf,
    /// Row ids of the current table after search/filter/sort (see `update`).
    ids: Vec<u64>,
    /// Duration of the previous frame, shown in the status bar.
    frame_ms: f32,
    #[cfg(not(target_arch = "wasm32"))]
    thumbs: images::Thumbs,
    #[cfg(not(target_arch = "wasm32"))]
    similar: similar::Similar,
    #[cfg(not(target_arch = "wasm32"))]
    people: people::People,
    #[cfg(not(target_arch = "wasm32"))]
    tagger: tagger::Tagger,
    #[cfg(not(target_arch = "wasm32"))]
    preview: Option<Preview>,
    #[cfg(not(target_arch = "wasm32"))]
    picker: Option<Picker>,
    /// Per-column filter inputs (a, b); b is the range end.
    filters: Vec<(String, String)>,
    tx: std::sync::mpsc::Sender<Msg>,
    rx: std::sync::mpsc::Receiver<Msg>,
}

impl App {
    pub fn new(store: Store) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        App {
            engine: Engine::new(store),
            table: 0,
            query: Query::default(),
            selected: None,
            form: None,
            status: String::new(),
            on_save: None,
            persist: false,
            styled: false,
            col_widths: Default::default(),
            show_filters: false,
            print: None,
            #[cfg(not(target_arch = "wasm32"))]
            unused: None,
            base_dir: ".".into(),
            ids: vec![],
            frame_ms: 0.0,
            #[cfg(not(target_arch = "wasm32"))]
            thumbs: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            similar: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            people: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            tagger: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            preview: None,
            #[cfg(not(target_arch = "wasm32"))]
            picker: None,
            filters: vec![],
            tx,
            rx,
        }
    }

    /// Show another table with a clean query and no selection.
    fn switch_table(&mut self, table: usize) {
        self.table = table;
        self.query = Query::default();
        self.filters.clear();
        self.selected = None;
        self.form = None;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.picker = None;
        }
    }

    /// Enlargement table index and its `image_ref` field, if the schema defines them.
    #[cfg(not(target_arch = "wasm32"))]
    fn enlargement_target(&self) -> Option<(usize, usize)> {
        let s = &self.engine.store.schema;
        let ti = s.role_index(TableRole::Enlargement)?;
        Some((ti, s.tables[ti].image_ref_index()?))
    }

    /// Open the enlargement table's form for a new row whose source is `path`.
    #[cfg(not(target_arch = "wasm32"))]
    fn new_enlargement(&mut self, path: String) {
        let Some((ti, fi)) = self.enlargement_target() else { return };
        self.switch_table(ti);
        self.open_form(None);
        if let Some(form) = &mut self.form {
            form.texts[fi] = path;
        }
    }

    /// Image view for `path`: the first row (film tables come first) whose strip contains it.
    #[cfg(not(target_arch = "wasm32"))]
    fn locate_image(&self, path: &str) -> Option<Preview> {
        for (ti, t) in self.engine.store.schema.tables.iter().enumerate() {
            if t.images_index().is_none() {
                continue;
            }
            for r in &self.engine.store.tables[ti].rows {
                if let Some(n) = self.row_image_paths(ti, r.id).iter().position(|p| p == path) {
                    return Some(Preview::new(ti, r.id, n));
                }
            }
        }
        None
    }

    /// Show the enlargement table filtered to the rows made from `path`.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_enlargements(&mut self, path: String) {
        let Some((ti, fi)) = self.enlargement_target() else { return };
        self.switch_table(ti);
        self.filters.resize(self.engine.store.schema.tables[ti].fields.len(), Default::default());
        self.filters[fi].0 = path;
    }

    fn open_form(&mut self, id: Option<u64>) {
        let t = &self.engine.store.schema.tables[self.table];
        let values: Vec<Value> = match id {
            Some(id) => self.engine.store.tables[self.table]
                .rows
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.values.clone())
                .unwrap_or_default(),
            None => t.fields.iter().map(default_value).collect(),
        };
        let texts = t.fields.iter().zip(&values).map(|(f, v)| format_value(v, f)).collect();
        self.form = Some(Form { id, texts, errors: vec![] });
    }

    fn commit_form(&mut self) {
        let Some(form) = &mut self.form else { return };
        let t = self.engine.store.schema.tables[self.table].clone();
        let mut values = vec![];
        form.errors.clear();
        for (f, s) in t.fields.iter().zip(&form.texts) {
            let v = match parse_value(s, f) {
                Ok(v) => v,
                Err(e) => {
                    form.errors.push(e.to_string());
                    Value::Null
                }
            };
            if f.required && v == Value::Null && f.ty != FieldType::Id {
                form.errors.push(format!("'{}' is required", f.label()));
            }
            if let (Value::Int(_) | Value::Float(_), true) = (&v, f.min.is_some() || f.max.is_some()) {
                let x = match v { Value::Int(i) => i as f64, Value::Float(x) => x, _ => 0.0 };
                if f.min.is_some_and(|m| x < m) || f.max.is_some_and(|m| x > m) {
                    form.errors.push(format!("'{}' out of range", f.label()));
                }
            }
            values.push(v);
        }
        if !form.errors.is_empty() {
            return;
        }
        let name = t.name.as_str();
        match form.id {
            Some(id) => {
                self.engine.update(name, id, values);
            }
            None => self.selected = self.engine.insert(name, values),
        }
        self.form = None;
    }

    fn import_csv(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        spawn(async move {
            if let Some(h) = rfd::AsyncFileDialog::new().add_filter("CSV", &["csv"]).pick_file().await {
                let text = String::from_utf8_lossy(&h.read().await).into_owned();
                let _ = tx.send(Msg::Csv(text));
                ctx.request_repaint();
            }
        });
    }

    fn open_files(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        spawn(async move {
            let Some(hs) = rfd::AsyncFileDialog::new().add_filter("JSON", &["json"]).pick_files().await else { return };
            let (mut s, mut d) = (None, None);
            for h in hs {
                let text = String::from_utf8_lossy(&h.read().await).into_owned();
                if h.file_name().contains("schema") { s = Some(text) } else { d = Some(text) }
            }
            let _ = tx.send(match (s, d) {
                (Some(s), Some(d)) => Msg::Open(s, d),
                _ => Msg::Error("select both the .schema.json and .data.json file".into()),
            });
            ctx.request_repaint();
        });
    }

    fn download(&self) {
        let (s, d) = self.engine.store.save();
        spawn(async move {
            for (name, text) in [("filmarchiv.schema.json", s), ("filmarchiv.data.json", d)] {
                if let Some(h) = rfd::AsyncFileDialog::new().set_file_name(name).save_file().await {
                    let _ = h.write(text.as_bytes()).await;
                }
            }
        });
    }

    fn open_print(&mut self) {
        let t = &self.engine.store.schema.tables[self.table];
        let columns = (0..t.fields.len())
            .filter(|&i| t.fields[i].visible_in_table && !matches!(t.fields[i].ty, FieldType::Images | FieldType::ImageRef))
            .collect();
        self.print = Some(print::PrintOpts {
            layout: print::Layout::Table,
            landscape: false,
            title: t.label().to_string(),
            columns,
            selected_only: false,
        });
    }

    fn print_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut o) = self.print.take() else { return };
        let t = self.engine.store.schema.tables[self.table].clone();
        let (mut open, mut create) = (true, false);
        // A schema card is a fixed sheet layout: no orientation, title or column choice.
        let fixed = o.layout == print::Layout::Cards && t.card.is_some();
        egui::Window::new("Print").open(&mut open).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut o.layout, print::Layout::Table, "Table");
                ui.radio_value(&mut o.layout, print::Layout::Cards, "Cards");
            });
            ui.add_enabled(!fixed, egui::Checkbox::new(&mut o.landscape, "Landscape"));
            ui.add_enabled(self.selected.is_some(), egui::Checkbox::new(&mut o.selected_only, "Selected row only"));
            if fixed {
                ui.label("Film sheet layout from the schema (portrait, fixed size).");
            } else {
                ui.horizontal(|ui| {
                    ui.label("Title");
                    ui.text_edit_singleline(&mut o.title);
                });
            }
            ui.separator();
            for (i, f) in t.fields.iter().enumerate() {
                if fixed || matches!(f.ty, FieldType::Images | FieldType::ImageRef) {
                    continue;
                }
                let mut on = o.columns.contains(&i);
                if ui.checkbox(&mut on, f.label()).changed() {
                    o.columns.retain(|&c| c != i);
                    if on {
                        o.columns.push(i);
                        o.columns.sort_unstable();
                    }
                }
            }
            ui.separator();
            create = ui.add_enabled(fixed || !o.columns.is_empty(), egui::Button::new("Create PDF...")).clicked();
        });
        if create {
            let ids = match (o.selected_only, self.selected) {
                (true, Some(id)) => vec![id],
                _ => self.ids.clone(),
            };
            let card = t.card.as_ref().filter(|_| fixed);
            let (pdf, count) = match (card, print::card_rows(&self.engine.store, self.table, &ids)) {
                (Some(card), Some(rows)) => (print::render_cards_pdf(card, &rows), rows.len()),
                _ => {
                    let (headers, rows) = print::table_rows(&self.engine.store, self.table, &ids, &o.columns);
                    (print::render_pdf(&headers, &rows, &o), rows.len())
                }
            };
            let name = format!("{}.pdf", t.name);
            spawn(async move {
                if let Some(h) = rfd::AsyncFileDialog::new().set_file_name(name).save_file().await {
                    let _ = h.write(&pdf).await;
                }
            });
            self.status = format!("PDF with {count} row(s) created");
        } else if open {
            self.print = Some(o);
        }
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Csv(text) => {
                    let schema = self.engine.store.schema.clone();
                    let t = &schema.tables[self.table];
                    match TableData::from_csv(t, &text) {
                        Ok(td) => {
                            let n = td.rows.len();
                            for r in td.rows { self.engine.insert(&t.name, r.values); }
                            self.status = format!("Imported {n} rows");
                        }
                        Err(e) => self.status = format!("Import failed: {e}"),
                    }
                }
                Msg::Open(s, d) => match Store::load(&s, &d) {
                    Ok((store, w)) => {
                        self.engine = Engine::new(store);
                        self.table = 0;
                        self.selected = None;
                        self.status = format!("Opened. {}", w.join("; "));
                    }
                    Err(e) => self.status = format!("Open failed: {e}"),
                },
                Msg::Error(e) => self.status = e,
                #[cfg(not(target_arch = "wasm32"))]
                Msg::AddImages(p) => self.add_images(p),
                #[cfg(not(target_arch = "wasm32"))]
                Msg::Copied(table, id, new, errs) => self.finish_copy(table, id, new, errs),
            }
        }
    }

    fn save(&mut self) {
        self.status = match self.on_save.as_mut() {
            Some(f) => match f(&self.engine.store) {
                Ok(()) => {
                    self.engine.dirty = false;
                    "Saved".into()
                }
                Err(e) => format!("Save failed: {e}"),
            },
            None => "No save target".into(),
        };
    }

    /// Save pending changes, then close the window; stays open if saving fails.
    #[cfg(not(target_arch = "wasm32"))]
    fn quit(&mut self, ctx: &egui::Context) {
        if self.engine.dirty {
            self.save();
        }
        if !self.engine.dirty {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn field_editor(ui: &mut egui::Ui, f: &Field, text: &mut String) -> Option<f32> {
        let mut right = None;
        match f.ty {
            FieldType::Id => {
                ui.label(text.as_str());
            }
            FieldType::Images => {
                ui.weak(format!("{} image(s), managed in the strip below the table", text.lines().filter(|l| !l.trim().is_empty()).count()));
            }
            FieldType::Enum => {
                egui::ComboBox::from_id_salt(&f.name)
                    .selected_text(text.as_str())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(text, String::new(), "");
                        for o in &f.options {
                            ui.selectable_value(text, o.clone(), o);
                        }
                    });
            }
            FieldType::Bool => {
                let mut b = text == "true";
                if ui.checkbox(&mut b, "").changed() {
                    *text = b.to_string();
                }
            }
            _ if f.multiline => {
                right = Some(ui.add(egui::TextEdit::multiline(text).desired_rows(3).desired_width(300.0)).rect.right());
            }
            _ => {
                let hint = match f.ty {
                    FieldType::Date => f.date_format().replace("%d", "dd").replace("%m", "mm").replace("%Y", "yyyy"),
                    FieldType::Time => f.time_format().replace("%M", "mm").replace("%S", "ss").replace("%H", "hh"),
                    _ => String::new(),
                };
                right = Some(ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(300.0)).rect.right());
                if let Some(u) = &f.unit {
                    ui.label(u);
                }
            }
        }
        right
    }

    /// Index of the table's first `images` field.
    #[cfg(not(target_arch = "wasm32"))]
    fn image_field(&self) -> Option<usize> {
        self.engine.store.schema.tables[self.table].fields.iter().position(|f| f.ty == FieldType::Images)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn selected_paths(&self) -> Vec<String> {
        let (Some(fi), Some(id)) = (self.image_field(), self.selected) else { return vec![] };
        self.engine.store.tables[self.table]
            .rows
            .iter()
            .find(|r| r.id == id)
            .map(|r| fa_data::image_paths(&r.values[fi]))
            .unwrap_or_default()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn set_selected_paths(&mut self, paths: Vec<String>) {
        let (Some(fi), Some(id)) = (self.image_field(), self.selected) else { return };
        let name = self.engine.store.schema.tables[self.table].name.clone();
        let Some(row) = self.engine.store.tables[self.table].rows.iter().find(|r| r.id == id) else { return };
        let mut values = row.values.clone();
        values[fi] = if paths.is_empty() { Value::Null } else { Value::Text(paths.join("\n")) };
        self.engine.update(&name, id, values);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn add_images(&mut self, files: Vec<std::path::PathBuf>) {
        let (Some(id), true) = (self.selected, self.image_field().is_some()) else {
            self.status = "Select a row first to add images".into();
            return;
        };
        let table = self.table;
        let name = self.engine.store.schema.tables[table].name.clone();
        let (dir, tx) = (self.base_dir.clone(), self.tx.clone());
        self.status = format!("Copying {} file(s)...", files.len());
        std::thread::spawn(move || {
            let (mut ok, mut errs) = (vec![], vec![]);
            for f in files {
                match fa_data::fs::import_image(&dir, &name, id, &f) {
                    Ok(p) => ok.push(p),
                    Err(e) => errs.push(format!("{}: {e}", f.display())),
                }
            }
            let _ = tx.send(Msg::Copied(table, id, ok, errs));
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn finish_copy(&mut self, table: usize, id: u64, new: Vec<String>, errs: Vec<String>) {
        let n = new.len();
        self.thumbs.forget_missing();
        if table == self.table && self.selected == Some(id) {
            let mut paths = self.selected_paths();
            for p in new {
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
            self.set_selected_paths(paths);
        }
        self.status = if errs.is_empty() { format!("Copied {n} file(s)") } else { format!("Copied {n}, failed: {}", errs.join("; ")) };
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn resolve(&self, p: &str) -> std::path::PathBuf {
        let p = std::path::Path::new(p);
        if p.is_absolute() { p.to_path_buf() } else { self.base_dir.join(p) }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn all_image_paths(&self) -> Vec<String> {
        let s = &self.engine.store;
        let mut out = vec![];
        for (t, data) in s.schema.tables.iter().zip(&s.tables) {
            if let Some(fi) = t.images_index() {
                for r in &data.rows {
                    out.extend(fa_data::image_paths(&r.values[fi]));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Rotates the image file `rel` by `turns` quarter turns (clockwise) and reloads its thumbnails.
    #[cfg(not(target_arch = "wasm32"))]
    fn rotate_image(&mut self, rel: &str, turns: i32) {
        let full = self.resolve(rel);
        self.status = match images::rotate_file(&full, turns) {
            Ok(()) => {
                self.thumbs.invalidate(&full);
                format!("Rotated {rel}")
            }
            Err(e) => format!("Rotate failed: {e}"),
        };
    }

    /// Crops the image file `rel` to `rect` (fractions of its size), keeping the original in `.originals`.
    #[cfg(not(target_arch = "wasm32"))]
    fn crop_image(&mut self, rel: &str, rect: [f32; 4]) {
        let full = self.resolve(rel);
        self.status = match images::backup_original(&self.base_dir, &full).and_then(|()| images::crop_file(&full, rect)) {
            Ok(()) => {
                self.thumbs.invalidate(&full);
                format!("Cropped {rel}")
            }
            Err(e) => format!("Crop failed: {e}"),
        };
    }

    /// Puts back the original of the image file `rel`.
    #[cfg(not(target_arch = "wasm32"))]
    fn restore_image(&mut self, rel: &str) {
        let full = self.resolve(rel);
        self.status = match images::restore_original(&self.base_dir, &full) {
            Ok(()) => {
                self.thumbs.invalidate(&full);
                format!("Restored original of {rel}")
            }
            Err(e) => format!("Restore failed: {e}"),
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn image_strip(&mut self, ctx: &egui::Context) {
        if self.image_field().is_none() {
            return;
        }
        let own = self.selected_paths();
        let skip = self.source_tiles();
        let paths = self.selected.map(|id| self.row_image_paths(self.table, id)).unwrap_or_default();
        let mut remove = None;
        let mut add = false;
        // Enlargements per image; only film tables link to the enlargement table.
        let linkable = self.engine.store.schema.tables[self.table].role == Some(TableRole::Film) && self.enlargement_target().is_some();
        let counts: Vec<usize> = paths.iter().map(|p| if linkable { fa_data::enlargements_for(&self.engine.store, p).len() } else { 0 }).collect();
        let (mut new_enl, mut show_enl, mut find_similar) = (None, None, None);
        let mut rotate: Option<(String, i32)> = None;
        egui::TopBottomPanel::bottom("images").resizable(true).default_height(150.0).min_height(60.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Images").strong());
                if ui.add_enabled(self.selected.is_some(), egui::Button::new("Add images...")).clicked() {
                    add = true;
                }
                if self.selected.is_none() {
                    ui.weak("select a row to see its images (or drop files here)");
                }
            });
            let size = (ui.available_height() - ui.spacing().scroll.allocated_width() - 4.0).clamp(48.0, 400.0);
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                if ui.rect_contains_pointer(ui.clip_rect()) {
                    let d = ctx.input(|i| i.smooth_scroll_delta);
                    ui.scroll_with_delta(egui::vec2(d.y, 0.0));
                }
                ui.horizontal_top(|ui| {
                    for (n, p) in paths.iter().enumerate() {
                        let full = self.resolve(p);
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(size * 1.3, size), egui::Sense::click());
                        if ui.is_rect_visible(rect) {
                            self.thumbs.paint_cropped(ui, ctx, &full, rect, 320);
                        }
                        let is_source = n < skip;
                        if is_source {
                            overlay_pill(ui.painter(), rect.left_top(), false, "Negativ");
                        }
                        if counts[n] > 0 {
                            overlay_pill(ui.painter(), rect.right_top(), true, &counts[n].to_string());
                        }
                        if resp.double_clicked() {
                            self.preview = self.selected.map(|row| Preview::new(self.table, row, n));
                        }
                        resp.on_hover_text(p).context_menu(|ui| {
                            if ui.button("Open large").clicked() {
                                self.preview = self.selected.map(|row| Preview::new(self.table, row, n));
                                ui.close_menu();
                            }
                            if linkable {
                                if ui.button("New enlargement").clicked() {
                                    new_enl = Some(p.clone());
                                    ui.close_menu();
                                }
                                if ui.add_enabled(counts[n] > 0, egui::Button::new(format!("Show enlargements ({})", counts[n]))).clicked() {
                                    show_enl = Some(p.clone());
                                    ui.close_menu();
                                }
                            }
                            if ui.button("Find similar images").clicked() {
                                find_similar = Some(p.clone());
                                ui.close_menu();
                            }
                            if ui.button("Rotate left").clicked() {
                                rotate = Some((p.clone(), -1));
                                ui.close_menu();
                            }
                            if ui.button("Rotate right").clicked() {
                                rotate = Some((p.clone(), 1));
                                ui.close_menu();
                            }
                            if !is_source && ui.button("Remove").clicked() {
                                remove = Some(n - skip);
                                ui.close_menu();
                            }
                        });
                        if n + 1 == skip {
                            ui.separator();
                        }
                    }
                });
            });
        });
        if let Some(p) = new_enl {
            self.new_enlargement(p);
            return;
        }
        if let Some((p, turns)) = rotate {
            self.rotate_image(&p, turns);
        }
        if let Some(q) = find_similar {
            let all = self.all_image_paths();
            self.similar.start(ctx, self.base_dir.clone(), all, q);
        }
        if let Some(p) = show_enl {
            self.show_enlargements(p);
            return;
        }
        if let Some(n) = remove {
            let mut p = own;
            p.remove(n);
            self.set_selected_paths(p);
        }
        if add {
            let (tx, ctx) = (self.tx.clone(), ctx.clone());
            spawn(async move {
                let files = rfd::AsyncFileDialog::new()
                    .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"])
                    .pick_files()
                    .await;
                if let Some(fs) = files {
                    let _ = tx.send(Msg::AddImages(fs.iter().map(|h| h.path().to_path_buf()).collect()));
                    ctx.request_repaint();
                }
            });
        }
    }

    /// Pick a film, then one of its images, as the source of the enlargement being edited.
    #[cfg(not(target_arch = "wasm32"))]
    fn picker_window(&mut self, ctx: &egui::Context) {
        let Some(mut pk) = self.picker.take() else { return };
        let Some((_, fi)) = self.enlargement_target() else { return };
        let schema = self.engine.store.schema.clone();
        let films: Vec<usize> = (0..schema.tables.len()).filter(|&i| schema.tables[i].role == Some(TableRole::Film)).collect();
        let (mut open, mut chosen) = (true, None);
        egui::Window::new("Choose source image").open(&mut open).default_size([520.0, 420.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Film table");
                egui::ComboBox::from_id_salt("pick_table").selected_text(schema.tables[pk.table].label()).show_ui(ui, |ui| {
                    for &i in &films {
                        if ui.selectable_value(&mut pk.table, i, schema.tables[i].label()).clicked() {
                            pk.row = None;
                        }
                    }
                });
            });
            let t = &schema.tables[pk.table];
            let td = &self.engine.store.tables[pk.table];
            let label_cols: Vec<usize> = (0..t.fields.len()).filter(|&i| t.fields[i].visible_in_table && t.fields[i].ty != FieldType::Id).take(3).collect();
            egui::ScrollArea::vertical().id_salt("pick_rows").max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
                for r in &td.rows {
                    let text = label_cols.iter().map(|&i| format_value(&r.values[i], &t.fields[i])).collect::<Vec<_>>().join("  ");
                    if ui.selectable_label(pk.row == Some(r.id), format!("#{}  {text}", r.id)).clicked() {
                        pk.row = Some(r.id);
                    }
                }
            });
            ui.separator();
            let paths = match (t.images_index(), pk.row.and_then(|id| td.rows.iter().find(|r| r.id == id))) {
                (Some(ii), Some(r)) => fa_data::image_paths(&r.values[ii]),
                _ => vec![],
            };
            if pk.row.is_some() && paths.is_empty() {
                ui.weak("This film has no images.");
            }
            egui::ScrollArea::horizontal().id_salt("pick_imgs").show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for p in &paths {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(150.0, 110.0), egui::Sense::click());
                        let full = std::path::Path::new(p).to_path_buf();
                        let full = if full.is_absolute() { full } else { self.base_dir.join(full) };
                        self.thumbs.paint(ui, ctx, &full, rect, 320);
                        if resp.hovered() {
                            ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(70, 130, 200)), egui::StrokeKind::Inside);
                        }
                        if resp.on_hover_text(p).clicked() {
                            chosen = Some(p.clone());
                        }
                    }
                });
            });
        });
        if let Some(p) = chosen {
            if let Some(form) = &mut self.form {
                form.texts[fi] = p;
            }
        } else if open {
            self.picker = Some(pk);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn unused_dialog(&mut self, ctx: &egui::Context) {
        let Some(files) = self.unused.clone() else { return };
        let (mut delete, mut cancel) = (false, false);
        egui::Window::new("Delete unused images").collapsible(false).show(ctx, |ui| {
            if files.is_empty() {
                ui.label("No unused images.");
            } else {
                ui.label(format!("{} file(s) are not used by any row:", files.len()));
                egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                    for f in &files {
                        ui.label(f.strip_prefix(&self.base_dir).unwrap_or(f).to_string_lossy());
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if files.is_empty() {
                    cancel = ui.button("OK").clicked();
                } else {
                    delete = ui.button("Delete").clicked();
                    cancel = ui.button("Cancel").clicked();
                }
            });
        });
        if delete {
            // Re-check: only delete what is still unused now.
            let still: std::collections::HashSet<_> = fa_data::fs::unused_images(&self.engine.store, &self.base_dir).into_iter().collect();
            let files: Vec<_> = files.into_iter().filter(|f| still.contains(f)).collect();
            let failed = files.iter().filter(|f| std::fs::remove_file(f).is_err()).count();
            self.status = format!("Deleted {} file(s), {failed} failed", files.len() - failed);
        }
        if delete || cancel {
            self.unused = None;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn preview_paths(&self, pv: &Preview) -> Vec<String> {
        self.row_image_paths(pv.table, pv.row)
    }

    /// Images shown for a row: for an enlargement its Quellbild first, then its own images.
    #[cfg(not(target_arch = "wasm32"))]
    fn row_image_paths(&self, table: usize, id: u64) -> Vec<String> {
        let t = &self.engine.store.schema.tables[table];
        let Some(fi) = t.images_index() else { return vec![] };
        let Some(row) = self.engine.store.tables[table].rows.iter().find(|r| r.id == id) else { return vec![] };
        let mut paths = vec![];
        if t.role == Some(TableRole::Enlargement) {
            if let Some(Value::Text(s)) = t.image_ref_index().map(|i| &row.values[i]) {
                if !s.trim().is_empty() {
                    paths.push(s.trim().to_string());
                }
            }
        }
        paths.extend(fa_data::image_paths(&row.values[fi]));
        paths
    }

    /// Number of leading strip tiles that are not the row's own images (the Quellbild).
    #[cfg(not(target_arch = "wasm32"))]
    fn source_tiles(&self) -> usize {
        let Some(id) = self.selected else { return 0 };
        self.row_image_paths(self.table, id).len() - self.selected_paths().len()
    }

    /// Images that have tags, with their tags.
    #[cfg(not(target_arch = "wasm32"))]
    fn tagged_images(&self) -> Vec<(String, Vec<String>)> {
        let store = &self.engine.store;
        let Some(ti) = store.schema.role_index(TableRole::ImageInfo) else { return vec![] };
        let t = &store.schema.tables[ti];
        let (Some(rf), Some(tg)) = (t.image_ref_index(), t.field_index("tags")) else { return vec![] };
        let text = |v: &Value| if let Value::Text(s) = v { s.clone() } else { String::new() };
        store.tables[ti]
            .rows
            .iter()
            .map(|r| (text(&r.values[rf]), fa_vision::split_tags(&text(&r.values[tg]))))
            .filter(|(p, t)| !p.is_empty() && !t.is_empty())
            .collect()
    }

    /// (tags, location) stored for the image `rel`.
    #[cfg(not(target_arch = "wasm32"))]
    fn image_info(&self, rel: &str) -> (String, String) {
        let store = &self.engine.store;
        let Some(ti) = store.schema.role_index(TableRole::ImageInfo) else { return Default::default() };
        let t = &store.schema.tables[ti];
        let (Some(tg), Some(lc), Some(id)) = (t.field_index("tags"), t.field_index("location"), fa_data::image_info_for(store, rel)) else { return Default::default() };
        let Some(row) = store.tables[ti].rows.iter().find(|r| r.id == id) else { return Default::default() };
        let text = |v: &Value| if let Value::Text(s) = v { s.clone() } else { String::new() };
        (text(&row.values[tg]), text(&row.values[lc]))
    }

    /// Store tags and location of image `rel`; creates the `image_info` row on first non-empty edit.
    #[cfg(not(target_arch = "wasm32"))]
    fn commit_info(&mut self, rel: &str, tags: &str, location: &str) {
        let store = &self.engine.store;
        let Some(ti) = store.schema.role_index(TableRole::ImageInfo) else { return };
        let t = &store.schema.tables[ti];
        let (Some(rf), Some(tg), Some(lc)) = (t.image_ref_index(), t.field_index("tags"), t.field_index("location")) else { return };
        let name = t.name.clone();
        let text = |s: &str| if s.trim().is_empty() { Value::Null } else { Value::Text(s.trim().to_string()) };
        match fa_data::image_info_for(store, rel) {
            Some(id) => {
                let Some(row) = store.tables[ti].rows.iter().find(|r| r.id == id) else { return };
                let mut values = row.values.clone();
                values[tg] = text(tags);
                values[lc] = text(location);
                self.engine.update(&name, id, values);
            }
            None if tags.trim().is_empty() && location.trim().is_empty() => {}
            None => {
                let mut values = TableData::blank_values(t);
                values[rf] = Value::Text(rel.to_string());
                values[tg] = text(tags);
                values[lc] = text(location);
                self.engine.insert(&name, values);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(mut pv) = self.preview.take() else { return };
        let paths = self.preview_paths(&pv);
        if paths.is_empty() {
            return;
        }
        pv.index = pv.index.min(paths.len() - 1);
        let rel = paths[pv.index].clone();
        if pv.loaded.as_deref() != Some(rel.as_str()) {
            (pv.tags, pv.location) = self.image_info(&rel);
            pv.loaded = Some(rel.clone());
        }
        let full = self.resolve(&rel);
        let (mut step, mut close) = (0i32, false);
        if !ctx.wants_keyboard_input() {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowLeft) {
                    step = -1;
                }
                if i.key_pressed(egui::Key::ArrowRight) {
                    step = 1;
                }
                close = i.key_pressed(egui::Key::Escape);
            });
        }
        // Keep showing the previous image (with its own histogram and markers) until this one is decoded.
        let ready = match self.thumbs.get(ctx, &full, 2048) {
            images::Thumb::Ready(t) => Some(t.clone()),
            _ => None,
        };
        let loading = ready.is_none();
        if let Some(t) = ready {
            let stats = self.thumbs.stats(&full, 2048).map(|s| (s.hist, s.black, s.white));
            pv.shown = Some((t, stats));
        }
        // Decode the next image in the direction of travel once this one is on screen, so Back/Next is quick.
        let ahead = if pv.dir < 0 { pv.index.checked_sub(1) } else { Some(pv.index + 1) };
        if let Some(j) = ahead.filter(|&j| !loading && j < paths.len()) {
            let p = self.resolve(&paths[j]);
            self.thumbs.preload(ctx, &p, 2048);
        }
        let tex = pv.shown.as_ref().map(|(t, _)| (t.id(), t.size_vec2()));
        let stats = pv.shown.as_ref().and_then(|(_, s)| *s);
        let table = &self.engine.store.schema.tables[pv.table];
        let heading = format!("{} #{}", table.label(), pv.row);
        let details: Vec<(String, String)> = self.engine.store.tables[pv.table]
            .rows
            .iter()
            .find(|r| r.id == pv.row)
            .map(|r| {
                table
                    .fields
                    .iter()
                    .zip(&r.values)
                    .filter(|(f, _)| f.ty != FieldType::Images)
                    .map(|(f, v)| (f.label.clone().unwrap_or_else(|| f.name.clone()), format_value(v, f)))
                    .filter(|(_, v)| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        const SIDE: f32 = 256.0;
        // Minimum height of the side column, and the margin below it that the image area adds.
        const SIDE_MIN: f32 = 520.0;
        const MARGIN: f32 = 6.0;
        /// Width of the image area relative to its height, so landscape photos fill it better.
        const LANDSCAPE: f32 = 1.3;
        // Image area (width = LANDSCAPE x height): as tall as the side column was in the last frame (tag chips included) plus a small margin.
        let edge = (pv.side_h.max(SIDE_MIN) + MARGIN).min((ctx.screen_rect().height() - 120.0).max(SIDE_MIN));
        let view = egui::vec2(edge * LANDSCAPE, edge);
        self.tagger.poll();
        let chips: Vec<(String, f32)> = match &self.tagger.result {
            Some((q, r)) if *q == rel => {
                let have: Vec<String> = fa_vision::split_tags(&pv.tags).iter().map(|t| t.to_lowercase()).collect();
                r.iter().filter(|(t, _)| !have.contains(&t.to_lowercase())).take(8).cloned().collect()
            }
            _ => vec![],
        };
        let (running, tag_error) = (self.tagger.running, self.tagger.error.clone());
        let searched = matches!(&self.tagger.result, Some((q, _)) if *q == rel);
        let (mut auto_tags, mut add_tag) = (false, None::<String>);
        let mut rotate = 0;
        let (mut apply_crop, mut restore) = (None::<[f32; 4]>, false);
        let has_original = images::has_original(&self.base_dir, &full);
        let (mut open, mut commit) = (true, false);
        egui::Window::new(full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .id(egui::Id::new("image_preview_v2"))
            .open(&mut open)
            .auto_sized()
            .show(ctx, |ui| {
                ui.set_max_width(view.x + SIDE + ui.spacing().item_spacing.x);
                ui.horizontal(|ui| {
                    if ui.add_enabled(pv.index > 0, egui::Button::new("◀ Back")).clicked() {
                        step = -1;
                    }
                    ui.label(format!("{} / {}", pv.index + 1, paths.len()));
                    if ui.add_enabled(pv.index + 1 < paths.len(), egui::Button::new("Next ▶")).clicked() {
                        step = 1;
                    }
                    ui.separator();
                    if ui.add_enabled(pv.zoom > 0.34, egui::Button::new("verkleinern")).clicked() {
                        pv.zoom = (pv.zoom * 0.8).max(0.333);
                    }
                    if ui.button("anpassen").clicked() {
                        pv.zoom = 1.0;
                    }
                    if ui.add_enabled(pv.zoom < 2.99, egui::Button::new("vergrössern")).clicked() {
                        pv.zoom = (pv.zoom * 1.25).min(3.0);
                    }
                    ui.weak(format!("{:.0}%", pv.zoom * 100.0));
                    ui.separator();
                    if ui.button("⟲").on_hover_text("Rotate left").clicked() {
                        rotate = -1;
                    }
                    if ui.button("⟳").on_hover_text("Rotate right").clicked() {
                        rotate = 1;
                    }
                    ui.separator();
                    if pv.crop.is_none() {
                        if ui.button("Crop").on_hover_text("Drag on the image to cut off borders").clicked() {
                            pv.crop = Some([0.0, 0.0, 1.0, 1.0]);
                        }
                    } else {
                        if ui.button("Apply crop").clicked() {
                            apply_crop = pv.crop;
                        }
                        if ui.button("Cancel").clicked() {
                            pv.crop = None;
                        }
                    }
                    if ui.add_enabled(has_original, egui::Button::new("Restore original")).on_hover_text("Undo crop and other edits").clicked() {
                        restore = true;
                    }
                });
                ui.separator();
                ui.horizontal_top(|ui| {
                    ui.allocate_ui(view, |ui| {
                        egui::ScrollArea::both().id_salt(&rel).max_width(view.x).max_height(view.y).auto_shrink([false, false]).drag_to_scroll(pv.crop.is_none()).show(ui, |ui| {
                            let Some((id, s)) = tex else {
                                ui.spinner();
                                return;
                            };
                            let fit = (view.x / s.x).min(view.y / s.y) * 0.99;
                            let img = s * fit * pv.zoom;
                            let (cell, resp) = ui.allocate_exact_size(img.max(view), egui::Sense::click_and_drag());
                            let rect = egui::Rect::from_center_size(cell.center(), img);
                            if resp.hovered() {
                                let (z, wheel) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.y));
                                let z = z * (wheel * 0.002).exp();
                                if z != 1.0 {
                                    pv.zoom = (pv.zoom * z).clamp(0.333, 3.0);
                                }
                                if wheel != 0.0 {
                                    ctx.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
                                }
                            }
                            if resp.double_clicked() {
                                pv.zoom = 1.0;
                            }
                            let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
                            painter.image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                            if let Some((_, black, white)) = &stats {
                                if pv.black {
                                    annotate(&painter, rect, *black, "Schwarz", false);
                                }
                                if pv.white {
                                    annotate(&painter, rect, *white, "Weiss", true);
                                }
                            }
                            if let Some(crop) = pv.crop.as_mut() {
                                crop_overlay(ui, &resp, &painter, rect, crop, &mut pv.grab);
                            }
                            if loading {
                                ui.put(egui::Rect::from_min_size(ui.clip_rect().min + egui::vec2(8.0, 8.0), egui::vec2(20.0, 20.0)), egui::Spinner::new());
                            }
                        });
                    });
                    let side = ui.vertical(|ui| {
                        ui.set_width(SIDE);
                        ui.set_min_height(SIDE_MIN);
                        ui.label(egui::RichText::new(heading).strong());
                        egui::Grid::new("film_info").num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
                            for (k, v) in &details {
                                ui.weak(k);
                                ui.add(egui::Label::new(v).wrap());
                                ui.end_row();
                            }
                        });
                        ui.add_space(8.0);
                        match &stats {
                            Some((h, _, _)) => histogram::paint(ui, h, egui::vec2(SIDE, 100.0)),
                            None => {
                                ui.allocate_space(egui::vec2(SIDE, 100.0));
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut pv.black, "Schwarz");
                            ui.checkbox(&mut pv.white, "Weiss");
                        });
                        ui.add_space(8.0);
                        ui.label("Tags");
                        commit |= ui.add(egui::TextEdit::multiline(&mut pv.tags).desired_rows(3).desired_width(f32::INFINITY)).lost_focus();
                        let row_h = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
                        // Fixed height of five rows: the button plus eight chips (about two per row) fit (scrolls if more): the column must not change size when suggestions appear.
                        let area_h = 5.0 * row_h;
                        egui::ScrollArea::vertical().id_salt("tag_chips").min_scrolled_height(area_h).max_height(area_h).auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if ui.add_enabled(!running, egui::Button::new(if running { "Auto tags ..." } else { "Auto tags" })).clicked() {
                                    auto_tags = true;
                                }
                                for (t, s) in &chips {
                                    if ui.button(format!("{t} {s:.2}")).on_hover_text("Add tag").clicked() {
                                        add_tag = Some(t.clone());
                                    }
                                }
                            });
                            if searched && chips.is_empty() && !running {
                                ui.weak("No new suggestions");
                            }
                            if let Some(e) = &tag_error {
                                ui.colored_label(egui::Color32::LIGHT_RED, e);
                            }
                        });
                        ui.add_space(4.0);
                        ui.label("Ort");
                        commit |= ui.add(egui::TextEdit::singleline(&mut pv.location).desired_width(f32::INFINITY)).lost_focus();
                    });
                    let h = side.response.rect.height();
                    if (h - pv.side_h).abs() > 1.0 {
                        pv.side_h = h;
                        ctx.request_repaint();
                    }
                });
            });
        if let Some(t) = add_tag {
            let sep = if pv.tags.trim().is_empty() { "" } else if pv.tags.trim_end().ends_with(',') { " " } else { ", " };
            pv.tags = format!("{}{sep}{t}", pv.tags.trim_end());
            commit = true;
        }
        if rotate != 0 {
            self.rotate_image(&rel, rotate);
            // A crop rectangle drawn before the turn no longer fits.
            pv.crop = pv.crop.map(|_| [0.0, 0.0, 1.0, 1.0]);
        }
        if let Some(c) = apply_crop {
            self.crop_image(&rel, c);
            pv.crop = None;
        }
        if restore {
            self.restore_image(&rel);
            pv.crop = None;
        }
        if auto_tags {
            let tagged = self.tagged_images();
            self.tagger.start(ctx, self.base_dir.clone(), rel.clone(), tagged);
        }
        if commit || step != 0 || close || !open {
            self.commit_info(&rel, &pv.tags, &pv.location);
        }
        if close || !open {
            return;
        }
        if step != 0 {
            pv.index = (pv.index as i32 + step).clamp(0, paths.len() as i32 - 1) as usize;
            pv.dir = step;
            pv.zoom = 1.0;
            pv.crop = None;
            ctx.request_repaint();
        }
        self.preview = Some(pv);
    }

    fn clear_filters(&mut self) {
        self.filters.iter_mut().for_each(|f| *f = Default::default());
        self.query.search.clear();
    }

    fn filter_controls(&mut self, ui: &mut egui::Ui) {
        let t = self.engine.store.schema.tables[self.table].clone();
        self.filters.resize(t.fields.len(), Default::default());
        egui::Grid::new("filter_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for (i, f) in t.fields.iter().enumerate() {
                if !f.visible_in_table {
                    continue;
                }
                let (a, b) = &mut self.filters[i];
                ui.label(f.label());
                ui.horizontal(|ui| match f.ty {
                    FieldType::Enum | FieldType::Bool => {
                        let opts: Vec<String> = if f.ty == FieldType::Bool { vec!["true".into(), "false".into()] } else { f.options.clone() };
                        egui::ComboBox::from_id_salt(("flt", i)).width(70.0).selected_text(a.as_str()).show_ui(ui, |ui| {
                            ui.selectable_value(a, String::new(), "(all)");
                            for o in opts {
                                ui.selectable_value(a, o.clone(), o);
                            }
                        });
                    }
                    FieldType::Id | FieldType::Int | FieldType::Float | FieldType::Date | FieldType::Time => {
                        ui.add(egui::TextEdit::singleline(a).hint_text("from").desired_width(60.0));
                        ui.add(egui::TextEdit::singleline(b).hint_text("to").desired_width(60.0));
                    }
                    _ => {
                        ui.add(egui::TextEdit::singleline(a).desired_width(160.0));
                    }
                });
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        if ui.button("Clear filters").clicked() {
            self.clear_filters();
        }
    }

    fn sync_filters(&mut self) {
        let t = self.engine.store.schema.tables[self.table].clone();
        self.filters.resize(t.fields.len(), Default::default());
        self.query.column_filters.clear();
        self.query.equals.clear();
        self.query.ranges.clear();
        for (i, f) in t.fields.iter().enumerate() {
            let (a, b) = self.filters[i].clone();
            match f.ty {
                FieldType::Enum | FieldType::Bool => self.query.equals.push((i, a)),
                FieldType::Id | FieldType::Int | FieldType::Float | FieldType::Date | FieldType::Time => self.query.ranges.push((i, a, b)),
                _ if !a.is_empty() => self.query.column_filters.push((i, a)),
                _ => {}
            }
        }
    }

    fn table_view(&mut self, ui: &mut egui::Ui) {
        ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
        let t = self.engine.store.schema.tables[self.table].clone();
        let ids = std::mem::take(&mut self.ids);
        let cols: Vec<usize> =
            (0..t.fields.len()).filter(|&i| t.fields[i].visible_in_table).collect();
        let mut open = None;
        ui.style_mut().interaction.selectable_labels = false;
        let table = self.table;
        let mut builder = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .max_scroll_height(f32::INFINITY);
        for (n, &i) in cols.iter().enumerate() {
            let col = if n + 1 == cols.len() {
                Column::remainder()
            } else {
                match self.col_widths.get(&(table, i)) {
                    Some(&w) => Column::exact(w),
                    None => Column::auto(),
                }
            };
            builder = builder.column(col.at_least(50.0).clip(true).resizable(false));
        }
        builder
            .header(22.0, |mut h| {
                for &i in &cols {
                    h.col(|ui| {
                        let arrow = match self.query.sort {
                            Some((c, true)) if c == i => " ^",
                            Some((c, false)) if c == i => " v",
                            _ => "",
                        };
                        if ui.add(egui::Label::new(egui::RichText::new(format!("{}{arrow}", t.fields[i].label())).strong()).sense(egui::Sense::click())).clicked() {
                            self.query.sort = match self.query.sort {
                                Some((c, true)) if c == i => Some((i, false)),
                                Some((c, false)) if c == i => None,
                                _ => Some((i, true)),
                            };
                        }
                        // Resize handle on the right edge of the header cell only.
                        let r = ui.max_rect();
                        let grab = egui::Rect::from_x_y_ranges(r.right() - 3.0..=r.right() + 5.0, r.y_range());
                        let resp = ui.interact(grab, ui.id().with("resize"), egui::Sense::click_and_drag());
                        if resp.hovered() || resp.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                        }
                        if resp.double_clicked() {
                            self.col_widths.remove(&(table, i));
                        }
                        if resp.dragged() {
                            let w = self.col_widths.get(&(table, i)).copied().unwrap_or(r.width());
                            self.col_widths.insert((table, i), (w + resp.drag_delta().x).max(50.0));
                        }
                    });
                }
            })
            .body(|body| {
                let td: &TableData = &self.engine.store.tables[self.table];
                let by_id: std::collections::HashMap<u64, &fa_data::Row> = td.rows.iter().map(|r| (r.id, r)).collect();
                body.rows(20.0, ids.len(), |mut row| {
                    let id = ids[row.index()];
                    let Some(r) = by_id.get(&id) else { return };
                    row.set_selected(self.selected == Some(id));
                    for &i in &cols {
                        row.col(|ui| {
                            ui.label(format_value(&r.values[i], &t.fields[i]).replace('\n', " "));
                        });
                    }
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some(id);
                    }
                    if resp.double_clicked() {
                        open = Some(id);
                    }
                });
            });
        self.ids = ids;
        if let Some(id) = open {
            self.open_form(Some(id));
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.persist {
            let (s, d) = self.engine.store.save();
            storage.set_string("schema", s);
            storage.set_string("data", d);
        }
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        #[cfg(not(target_arch = "wasm32"))]
        let frame_start = std::time::Instant::now();
        if !self.styled {
            apply_theme(ctx);
            self.styled = true;
        }
        self.poll();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.thumbs.poll(ctx, &self.base_dir);
            let dropped: Vec<_> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|d| d.path.clone()).collect());
            if !dropped.is_empty() {
                self.add_images(dropped);
            }
        }
        let (undo, redo, save, print) = ctx.input(|i| {
            (
                i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                i.modifiers.command && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::P),
            )
        });
        if self.form.is_none() {
            if undo { self.engine.undo(); }
            if redo { self.engine.redo(); }
        }
        if save { self.save(); }
        #[cfg(not(target_arch = "wasm32"))]
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Q)) {
            self.quit(ctx);
        }
        if print { self.open_print(); }
        // Filtered/sorted row ids, computed once per frame for the table and the toolbar.
        let table_name = self.engine.store.schema.tables[self.table].name.clone();
        self.ids = self.engine.view(&table_name, &self.query);

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open...").clicked() { self.open_files(ctx); ui.close_menu(); }
                    if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() { self.save(); ui.close_menu(); }
                    if ui.button("Download").clicked() { self.download(); ui.close_menu(); }
                    if ui.add(egui::Button::new("Print...").shortcut_text("Ctrl+P")).clicked() { self.open_print(); ui.close_menu(); }
                    ui.separator();
                    if ui.button("Import CSV...").clicked() { self.import_csv(ctx); ui.close_menu(); }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if ui.button("Open archive folder").clicked() {
                            let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" }).arg(&self.base_dir).spawn();
                            ui.close_menu();
                        }
                        if ui.button("Delete unused images...").clicked() {
                            self.unused = Some(fa_data::fs::unused_images(&self.engine.store, &self.base_dir));
                            ui.close_menu();
                        }
                    }
                    if ui.button("Purge orphans").clicked() {
                        self.engine.store.purge_orphans();
                        self.engine.dirty = true;
                        self.status = "Orphans purged".into();
                        ui.close_menu();
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        ui.separator();
                        if ui.add(egui::Button::new("Quit").shortcut_text("Ctrl+Q")).clicked() {
                            self.quit(ctx);
                            ui.close_menu();
                        }
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.add_enabled(self.engine.can_undo(), egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() { self.engine.undo(); ui.close_menu(); }
                    if ui.add_enabled(self.engine.can_redo(), egui::Button::new("Redo").shortcut_text("Ctrl+Y")).clicked() { self.engine.redo(); ui.close_menu(); }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Filter...").clicked() { self.show_filters = true; ui.close_menu(); }
                    if ui.button("Clear filters").clicked() { self.clear_filters(); ui.close_menu(); }
                    #[cfg(not(target_arch = "wasm32"))]
                    if ui.button("People...").clicked() {
                        let all = self.all_image_paths();
                        self.people.start(ctx, self.base_dir.clone(), all);
                        ui.close_menu();
                    }
                });
            });
        });
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("New").clicked() { self.open_form(None); }
                if ui.add_enabled(self.selected.is_some(), egui::Button::new("Edit")).clicked() {
                    self.open_form(self.selected);
                }
                if ui.add_enabled(self.selected.is_some(), egui::Button::new("Delete")).clicked() {
                    let name = self.engine.store.schema.tables[self.table].name.clone();
                    if let Some(id) = self.selected.take() { self.engine.delete(&name, id); }
                }
                ui.separator();
                ui.label("Search");
                ui.text_edit_singleline(&mut self.query.search);
                let active = self.filters.iter().any(|(a, b)| !a.is_empty() || !b.is_empty());
                if ui.selectable_label(active, if active { "Filter... *" } else { "Filter..." }).clicked() {
                    self.show_filters = true;
                }
                let total = self.engine.store.tables[self.table].rows.len();
                ui.weak(format!("{} of {total} rows", self.ids.len()));
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            let n = self.engine.store.tables[self.table].rows.len();
            ui.label(format!("{n} rows{}  {}  ({:.0} ms/frame)", if self.engine.dirty { "  (unsaved)" } else { "" }, self.status, self.frame_ms));
        });
        let tab_frame = egui::Frame::new().fill(egui::Color32::from_gray(22)).inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 0 });
        egui::TopBottomPanel::top("tabs").frame(tab_frame).show(ctx, |ui| {
            let mut pick = self.table;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, t) in self.engine.store.schema.tables.iter().enumerate() {
                    let active = i == self.table;
                    let galley = ui.painter().layout_no_wrap(t.label().to_string(), egui::FontId::proportional(15.0), egui::Color32::WHITE);
                    let (rect, resp) = ui.allocate_exact_size(galley.size() + egui::vec2(28.0, 12.0), egui::Sense::click());
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    let (fill, color) = if active {
                        (egui::Color32::from_gray(52), egui::Color32::WHITE)
                    } else if resp.hovered() {
                        (egui::Color32::from_gray(38), egui::Color32::from_gray(200))
                    } else {
                        (egui::Color32::from_gray(26), egui::Color32::from_gray(140))
                    };
                    let top = egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 };
                    ui.painter().rect_filled(rect, top, fill);
                    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
                    if resp.clicked() { pick = i; }
                }
            });
            if pick != self.table {
                self.switch_table(pick);
            }
        });
        #[cfg(not(target_arch = "wasm32"))]
        self.image_strip(ctx);
        self.sync_filters();
        if self.show_filters {
            let mut open = true;
            egui::Window::new("Filter").open(&mut open).show(ctx, |ui| self.filter_controls(ui));
            self.show_filters = open;
            self.sync_filters();
        }
        self.print_dialog(ctx);
        egui::CentralPanel::default().show(ctx, |ui| self.table_view(ui));
        #[cfg(not(target_arch = "wasm32"))]
        self.preview_window(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        self.unused_dialog(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(p) = self.similar.show(ctx, &mut self.thumbs, &self.base_dir) {
            match self.locate_image(&p) {
                Some(pv) => self.preview = Some(pv),
                None => self.status = "Image not in any row".into(),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (rescan, clicked) = self.people.show(ctx, &mut self.thumbs, &self.base_dir);
            if rescan {
                let all = self.all_image_paths();
                self.people.start(ctx, self.base_dir.clone(), all);
            }
            if let Some(p) = clicked {
                match self.locate_image(&p) {
                    Some(pv) => self.preview = Some(pv),
                    None => self.status = "Image not in any row".into(),
                }
            }
        }

        let mut commit = false;
        let mut cancel = false;
        #[cfg(not(target_arch = "wasm32"))]
        let mut choose = false;
        if let Some(form) = self.form.as_mut() {
            let t = self.engine.store.schema.tables[self.table].clone();

            let title = if form.id.is_some() { "Edit" } else { "New" };
            egui::Window::new(format!("{title}: {}", t.label()))
                .collapsible(false)
                .show(ctx, |ui| {
                    let mut input_right: Option<f32> = None;
                    egui::Grid::new("form").num_columns(2).show(ui, |ui| {
                        for (f, text) in t.fields.iter().zip(&mut form.texts) {
                            ui.label(f.label());
                            ui.horizontal_top(|ui| {
                                #[cfg(not(target_arch = "wasm32"))]
                                if f.ty == FieldType::ImageRef {
                                    choose |= image_ref_editor(ui, ctx, &mut self.thumbs, &self.base_dir, text);
                                    return;
                                }
                                if let Some(r) = Self::field_editor(ui, f, text) {
                                    input_right = Some(input_right.map_or(r, |x| x.max(r)));
                                }
                            });
                            ui.end_row();
                        }
                    });
                    for e in &form.errors {
                        ui.colored_label(egui::Color32::RED, e);
                    }
                    // Align OK with the right edge of the 300px inputs.
                    let right = input_right.unwrap_or(ui.max_rect().right());
                    let r = ui.available_rect_before_wrap();
                    ui.allocate_ui_with_layout(egui::vec2((right - r.left()).max(0.0), ui.spacing().interact_size.y), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        commit = ui.button("OK").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { cancel = true; }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if choose {
                let film = self.engine.store.schema.tables.iter().position(|t| t.role == Some(TableRole::Film));
                self.picker = film.map(|table| Picker { table, row: None });
            }
            self.picker_window(ctx);
        }
        if commit { self.commit_form(); }
        if cancel { self.form = None; }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.frame_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
        }
    }
}
