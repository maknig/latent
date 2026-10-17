//! Generic egui front end: table view and edit form generated from the schema.
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use fa_data::{default_value, format_value, parse_value, Store, TableData, Value};
use fa_engine::{Engine, Query};
use fa_schema::{Field, FieldType};

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
}

impl App {
    pub fn new(store: Store) -> Self {
        App {
            engine: Engine::new(store),
            table: 0,
            query: Query::default(),
            selected: None,
            form: None,
            status: String::new(),
            on_save: None,
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

    fn table_view(&mut self, ui: &mut egui::Ui) {
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
                        if ui.button(format!("{}{arrow}", t.fields[i].label())).clicked() {
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
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
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

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() { self.save(); }
                if ui.add_enabled(self.engine.can_undo(), egui::Button::new("Undo")).clicked() { self.engine.undo(); }
                if ui.add_enabled(self.engine.can_redo(), egui::Button::new("Redo")).clicked() { self.engine.redo(); }
                ui.separator();
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
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            let n = self.engine.store.tables[self.table].rows.len();
            ui.label(format!("{n} rows{}  {}", if self.engine.dirty { "  (unsaved)" } else { "" }, self.status));
        });
        egui::SidePanel::left("tables").show(ctx, |ui| {
            let mut pick = self.table;
            for (i, t) in self.engine.store.schema.tables.iter().enumerate() {
                ui.selectable_value(&mut pick, i, t.label());
            }
            if pick != self.table {
                self.table = pick;
                self.query = Query::default();
                self.selected = None;
                self.form = None;
            }
        });
        egui::CentralPanel::default().show(ctx, |ui| self.table_view(ui));

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
