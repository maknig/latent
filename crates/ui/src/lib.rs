//! Generic egui front end: table view and edit form generated from the schema.
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use fa_data::{default_value, format_value, parse_value, Store, TableData, Value};
use fa_engine::{Engine, Query};
use fa_schema::{Field, FieldType};

#[cfg(not(target_arch = "wasm32"))]
mod images;

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
    let accent = egui::Color32::from_rgb(70, 130, 200);
    let mut v = egui::Visuals::dark();
    v.panel_fill = egui::Color32::from_gray(30);
    v.window_fill = egui::Color32::from_gray(36);
    v.extreme_bg_color = egui::Color32::from_gray(22);
    v.faint_bg_color = egui::Color32::from_gray(36);
    v.selection.bg_fill = accent;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(48));
    v.widgets.hovered.bg_fill = egui::Color32::from_gray(55);
    v.striped = true;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}

type SaveHook = Box<dyn FnMut(&Store) -> Result<(), String>>;

struct Form {
    id: Option<u64>,
    texts: Vec<String>,
    errors: Vec<String>,
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
    show_filters: bool,
    /// Pending "delete unused images" confirmation.
    #[cfg(not(target_arch = "wasm32"))]
    unused: Option<Vec<std::path::PathBuf>>,
    /// Directory image paths are relative to (the data file's folder).
    pub base_dir: std::path::PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    thumbs: images::Thumbs,
    #[cfg(not(target_arch = "wasm32"))]
    preview: Option<std::path::PathBuf>,
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
            show_filters: false,
            #[cfg(not(target_arch = "wasm32"))]
            unused: None,
            base_dir: ".".into(),
            #[cfg(not(target_arch = "wasm32"))]
            thumbs: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            preview: None,
            filters: vec![],
            tx,
            rx,
        }
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

    fn field_editor(ui: &mut egui::Ui, f: &Field, text: &mut String) {
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
                ui.add(egui::TextEdit::multiline(text).desired_rows(3).desired_width(300.0));
            }
            _ => {
                let hint = match f.ty {
                    FieldType::Date => f.date_format().replace("%d", "dd").replace("%m", "mm").replace("%Y", "yyyy"),
                    FieldType::Time => f.time_format().replace("%M", "mm").replace("%S", "ss").replace("%H", "hh"),
                    _ => String::new(),
                };
                ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(300.0));
                if let Some(u) = &f.unit {
                    ui.label(u);
                }
            }
        }
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
    fn image_strip(&mut self, ctx: &egui::Context) {
        if self.image_field().is_none() {
            return;
        }
        let paths = self.selected_paths();
        let mut remove = None;
        let mut add = false;
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
            let size = (ui.available_height() - 12.0).clamp(48.0, 400.0);
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (n, p) in paths.iter().enumerate() {
                        let full = self.resolve(p);
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(size * 1.3, size), egui::Sense::click());
                        match self.thumbs.get(ctx, &full, 320) {
                            images::Thumb::Ready(tex) => {
                                let s = tex.size_vec2();
                                let scale = (rect.width() / s.x).min(rect.height() / s.y);
                                let r = egui::Rect::from_center_size(rect.center(), s * scale);
                                ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                            }
                            images::Thumb::Loading => {
                                ui.put(rect, egui::Spinner::new());
                            }
                            images::Thumb::Missing => {
                                ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(45));
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "missing", egui::FontId::proportional(12.0), egui::Color32::GRAY);
                            }
                        }
                        if resp.double_clicked() {
                            self.preview = Some(full);
                        }
                        resp.on_hover_text(p).context_menu(|ui| {
                            if ui.button("Open large").clicked() {
                                self.preview = Some(self.resolve(p));
                                ui.close_menu();
                            }
                            if ui.button("Remove").clicked() {
                                remove = Some(n);
                                ui.close_menu();
                            }
                        });
                    }
                });
            });
        });
        if let Some(n) = remove {
            let mut p = paths;
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
            ui.horizontal(|ui| {
                if !files.is_empty() {
                    delete = ui.button("Delete").clicked();
                }
                cancel = ui.button(if files.is_empty() { "OK" } else { "Cancel" }).clicked();
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
    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(path) = self.preview.clone() else { return };
        let mut open = true;
        egui::Window::new(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .open(&mut open)
            .default_size([700.0, 500.0])
            .resizable(true)
            .show(ctx, |ui| {
                let avail = ui.available_size();
                if let images::Thumb::Ready(tex) = self.thumbs.get(ctx, &path, 2048) {
                    let s = tex.size_vec2();
                    let scale = (avail.x / s.x).min(avail.y / s.y);
                    ui.centered_and_justified(|ui| ui.image((tex.id(), s * scale)));
                } else {
                    ui.spinner();
                }
            });
        if !open {
            self.preview = None;
        }
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
                if !f.visible_in_table || f.ty == FieldType::Id {
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
                    FieldType::Int | FieldType::Float | FieldType::Date | FieldType::Time => {
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
                FieldType::Int | FieldType::Float | FieldType::Date | FieldType::Time => self.query.ranges.push((i, a, b)),
                _ if !a.is_empty() => self.query.column_filters.push((i, a)),
                _ => {}
            }
        }
    }

    fn table_view(&mut self, ui: &mut egui::Ui) {
        ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
        let t = self.engine.store.schema.tables[self.table].clone();
        let ids = self.engine.view(&t.name, &self.query);
        let cols: Vec<usize> =
            (0..t.fields.len()).filter(|&i| t.fields[i].visible_in_table).collect();
        let mut open = None;
        let mut builder = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .max_scroll_height(f32::INFINITY);
        for _ in &cols {
            builder = builder.column(Column::auto().at_least(50.0).clip(true).resizable(true));
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
                    });
                }
            })
            .body(|body| {
                let td: &TableData = &self.engine.store.tables[self.table];
                body.rows(20.0, ids.len(), |mut row| {
                    let id = ids[row.index()];
                    let Some(r) = td.rows.iter().find(|r| r.id == id) else { return };
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
        if !self.styled {
            apply_theme(ctx);
            self.styled = true;
        }
        self.poll();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.thumbs.poll(ctx);
            let dropped: Vec<_> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|d| d.path.clone()).collect());
            if !dropped.is_empty() {
                self.add_images(dropped);
            }
        }
        let (undo, redo, save) = ctx.input(|i| {
            (
                i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                i.modifiers.command && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                i.modifiers.command && i.key_pressed(egui::Key::S),
            )
        });
        if self.form.is_none() {
            if undo { self.engine.undo(); }
            if redo { self.engine.redo(); }
        }
        if save { self.save(); }

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open...").clicked() { self.open_files(ctx); ui.close_menu(); }
                    if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() { self.save(); ui.close_menu(); }
                    if ui.button("Download").clicked() { self.download(); ui.close_menu(); }
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
                });
                ui.menu_button("Edit", |ui| {
                    if ui.add_enabled(self.engine.can_undo(), egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() { self.engine.undo(); ui.close_menu(); }
                    if ui.add_enabled(self.engine.can_redo(), egui::Button::new("Redo").shortcut_text("Ctrl+Y")).clicked() { self.engine.redo(); ui.close_menu(); }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Filter...").clicked() { self.show_filters = true; ui.close_menu(); }
                    if ui.button("Clear filters").clicked() { self.clear_filters(); ui.close_menu(); }
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
                let name = self.engine.store.schema.tables[self.table].name.clone();
                ui.weak(format!("{} of {total} rows", self.engine.view(&name, &self.query).len()));
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            let n = self.engine.store.tables[self.table].rows.len();
            ui.label(format!("{n} rows{}  {}", if self.engine.dirty { "  (unsaved)" } else { "" }, self.status));
        });
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            let mut pick = self.table;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for (i, t) in self.engine.store.schema.tables.iter().enumerate() {
                    let active = i == self.table;
                    let color = if active { egui::Color32::WHITE } else { egui::Color32::from_gray(140) };
                    let resp = ui.add(egui::Label::new(egui::RichText::new(t.label()).size(15.0).color(color)).sense(egui::Sense::click()));
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if active {
                        let r = resp.rect;
                        ui.painter().line_segment([egui::pos2(r.left(), r.bottom() + 3.0), egui::pos2(r.right(), r.bottom() + 3.0)], egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(70, 130, 200)));
                    } else if resp.hovered() {
                        ui.painter().line_segment([egui::pos2(resp.rect.left(), resp.rect.bottom() + 3.0), egui::pos2(resp.rect.right(), resp.rect.bottom() + 3.0)], egui::Stroke::new(1.0_f32, egui::Color32::from_gray(90)));
                    }
                    if resp.clicked() { pick = i; }
                }
            });
            ui.add_space(3.0);
            if pick != self.table {
                self.table = pick;
                self.query = Query::default();
                self.filters.clear();
                self.selected = None;
                self.form = None;
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
        egui::CentralPanel::default().show(ctx, |ui| self.table_view(ui));
        #[cfg(not(target_arch = "wasm32"))]
        self.preview_window(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        self.unused_dialog(ctx);

        let mut commit = false;
        let mut cancel = false;
        if let Some(form) = self.form.as_mut() {
            let t = self.engine.store.schema.tables[self.table].clone();

            let title = if form.id.is_some() { "Edit" } else { "New" };
            egui::Window::new(format!("{title}: {}", t.label()))
                .collapsible(false)
                .show(ctx, |ui| {
                    egui::Grid::new("form").num_columns(2).show(ui, |ui| {
                        for (f, text) in t.fields.iter().zip(&mut form.texts) {
                            ui.label(f.label());
                            ui.horizontal(|ui| Self::field_editor(ui, f, text));
                            ui.end_row();
                        }
                    });
                    for e in &form.errors {
                        ui.colored_label(egui::Color32::RED, e);
                    }
                    ui.horizontal(|ui| {
                        commit = ui.button("OK").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { cancel = true; }
        }
        if commit { self.commit_form(); }
        if cancel { self.form = None; }
    }
}
