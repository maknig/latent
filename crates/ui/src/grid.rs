//! Thumbnail grid: one tile per row of the current table, with the same selection as the table view.
use crate::{format_value, App};
use eframe::egui;
use latent_data::{Row, Value};
use latent_schema::Table;

const TILE: egui::Vec2 = egui::vec2(160.0, 132.0);
const THUMB_H: f32 = 100.0;
const GAP: f32 = 6.0;

/// Archive-relative path of the image that stands for `row`: its first own image, else its linked image.
pub(crate) fn cover(t: &Table, row: &Row) -> Option<String> {
    if let Some(i) = t.images_index() {
        if let Some(p) = latent_data::image_paths(&row.values[i]).into_iter().next() {
            return Some(p);
        }
    }
    match t.image_ref_index().map(|i| &row.values[i]) {
        Some(Value::Text(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

impl App {
    pub(crate) fn grid_view(&mut self, ui: &mut egui::Ui) {
        let t = self.engine.store.schema.tables[self.table].clone();
        let ids = std::mem::take(&mut self.ids);
        let captions: Vec<usize> = (0..t.fields.len())
            .filter(|&i| t.fields[i].visible_in_table)
            .take(2)
            .collect();
        let per_row = (((ui.available_width() + GAP) / (TILE.x + GAP)) as usize).max(1);
        let lines = ids.len().div_ceil(per_row);
        let ctx = ui.ctx().clone();
        let base = self.base_dir.clone();
        let mut open = None;
        let td = &self.engine.store.tables[self.table];
        let by_id: std::collections::HashMap<u64, &Row> =
            td.rows.iter().map(|r| (r.id, r)).collect();
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            TILE.y,
            lines,
            |ui, range| {
                ui.spacing_mut().item_spacing = egui::vec2(GAP, 0.0);
                for line in range {
                    ui.horizontal(|ui| {
                        for &id in ids.iter().skip(line * per_row).take(per_row) {
                            let Some(r) = by_id.get(&id) else { continue };
                            let (rect, resp) = ui.allocate_exact_size(TILE, egui::Sense::click());
                            if !ui.is_rect_visible(rect) {
                                continue;
                            }
                            let selected = self.selected == Some(id);
                            if selected {
                                ui.painter()
                                    .rect_filled(rect, 4.0, ui.visuals().selection.bg_fill);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    4.0,
                                    ui.visuals().widgets.hovered.weak_bg_fill,
                                );
                            }
                            let pic = egui::Rect::from_min_size(
                                rect.min + egui::vec2(3.0, 3.0),
                                egui::vec2(TILE.x - 6.0, THUMB_H),
                            );
                            match cover(&t, r) {
                                Some(p) => {
                                    let path = base.join(&p);
                                    self.thumbs.paint_cropped(ui, &ctx, &path, pic, 320);
                                }
                                None => {
                                    ui.painter().rect_filled(
                                        pic,
                                        2.0,
                                        egui::Color32::from_gray(30),
                                    );
                                }
                            }
                            let text = captions
                                .iter()
                                .map(|&i| {
                                    format_value(&r.values[i], &t.fields[i]).replace('\n', " ")
                                })
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join("  ");
                            ui.painter().with_clip_rect(rect).text(
                                pic.left_bottom() + egui::vec2(0.0, 4.0),
                                egui::Align2::LEFT_TOP,
                                text,
                                egui::FontId::proportional(13.0),
                                ui.visuals().text_color(),
                            );
                            if resp.clicked() {
                                self.selected = Some(id);
                            }
                            if resp.double_clicked() {
                                open = Some(id);
                            }
                        }
                    });
                }
            },
        );
        self.ids = ids;
        if let Some(id) = open {
            self.open_form(Some(id));
        }
    }
}
