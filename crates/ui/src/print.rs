//! Printing: lays rows out as a table list or as film cards and writes a PDF.
//!
//! The PDF is written by hand (Courier, WinAnsi) so it needs no dependency and
//! works the same on native and wasm. Courier is monospaced, so text width is
//! exact (as in the Qt original, which also printed in Courier).

use latent_data::{format_value, Store};

const A4: (f32, f32) = (595.28, 841.89);
const MARGIN: f32 = 40.0;
const CHAR_W: f32 = 0.6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Table,
    Cards,
}

#[derive(Clone, Debug)]
pub struct PrintOpts {
    pub layout: Layout,
    pub landscape: bool,
    pub title: String,
    /// Field indices to print, in order.
    pub columns: Vec<usize>,
    /// Print only the selected row instead of all filtered rows.
    pub selected_only: bool,
}

/// Header labels and formatted cells for `ids` (in that order) and `cols`.
pub fn table_rows(
    store: &Store,
    table: usize,
    ids: &[u64],
    cols: &[usize],
) -> (Vec<String>, Vec<Vec<String>>) {
    let t = &store.schema.tables[table];
    let headers = cols
        .iter()
        .map(|&c| t.fields[c].label().to_string())
        .collect();
    let data = &store.tables[table];
    let by_id: std::collections::HashMap<u64, &latent_data::Row> =
        data.rows.iter().map(|r| (r.id, r)).collect();
    let rows = ids
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|r| {
            cols.iter()
                .map(|&c| format_value(&r.values[c], &t.fields[c]))
                .collect()
        })
        .collect();
    (headers, rows)
}

/// One page: a PDF content stream.
struct Page {
    ops: String,
}

impl Page {
    fn text(&mut self, x: f32, y: f32, size: f32, bold: bool, s: &str) {
        let font = if bold { "F2" } else { "F1" };
        self.ops.push_str(&format!(
            "BT /{font} {size:.1} Tf {x:.2} {y:.2} Td ({}) Tj ET\n",
            pdf_escape(s)
        ));
    }
    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, w: f32) {
        self.ops
            .push_str(&format!("{w:.2} w {x1:.2} {y1:.2} m {x2:.2} {y2:.2} l S\n"));
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, lw: f32) {
        self.ops
            .push_str(&format!("{lw:.2} w {x:.2} {y:.2} {w:.2} {h:.2} re S\n"));
    }
}

/// Escape for a PDF string; non-WinAnsi characters become `?`.
fn pdf_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let code = c as u32;
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            _ if (0xA0..=0xFF).contains(&code) => out.push_str(&format!("\\{code:03o}")),
            '€' => out.push_str("\\200"),
            _ => out.push('?'),
        }
    }
    out
}

/// Hard-wrap `s` (newlines count as spaces) into lines of at most `n` chars,
/// preferring to break at spaces.
fn wrap(s: &str, n: usize) -> Vec<String> {
    let n = n.max(1);
    let mut lines = vec![];
    for para in s.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let mut word: Vec<char> = word.chars().collect();
            let cur_len = cur.chars().count();
            if !cur.is_empty() && cur_len + 1 + word.len() <= n {
                cur.push(' ');
                cur.extend(word);
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            while word.len() > n {
                lines.push(word.drain(..n).collect());
            }
            cur = word.into_iter().collect();
        }
        lines.push(cur);
    }
    lines
}

fn page_size(landscape: bool) -> (f32, f32) {
    if landscape {
        (A4.1, A4.0)
    } else {
        A4
    }
}

fn layout_table(
    headers: &[String],
    rows: &[Vec<String>],
    title: &str,
    (pw, ph): (f32, f32),
) -> Vec<Page> {
    let avail = pw - 2.0 * MARGIN;
    let pad = 6.0;
    let mut widths: Vec<usize> = (0..headers.len())
        .map(|c| {
            let longest = rows
                .iter()
                .map(|r| r[c].lines().map(|l| l.chars().count()).max().unwrap_or(0))
                .max()
                .unwrap_or(0);
            longest.max(headers[c].chars().count()).clamp(3, 40)
        })
        .collect();
    let total = |w: &[usize], fs: f32| w.iter().map(|&n| n as f32 * CHAR_W * fs + pad).sum::<f32>();
    let mut fs = 9.0;
    while total(&widths, fs) > avail && fs > 6.0 {
        fs -= 0.5;
    }
    if total(&widths, fs) > avail {
        // Still too wide: shrink the columns proportionally; cells wrap.
        let chars =
            ((avail - pad * widths.len() as f32) / (CHAR_W * fs)).max(widths.len() as f32 * 3.0);
        let sum: usize = widths.iter().sum();
        let k = chars / sum as f32;
        for w in &mut widths {
            *w = ((*w as f32 * k) as usize).max(3);
        }
    }
    let lh = fs * 1.25;
    let col_x: Vec<f32> = widths
        .iter()
        .scan(MARGIN, |x, &n| {
            let at = *x;
            *x += n as f32 * CHAR_W * fs + pad;
            Some(at)
        })
        .collect();
    let right = MARGIN + total(&widths, fs);
    let top = ph - MARGIN;
    let bottom = MARGIN + 14.0;

    let mut pages: Vec<Page> = vec![];
    let mut y = 0.0;
    let new_page = |pages: &mut Vec<Page>, y: &mut f32| {
        let mut p = Page { ops: String::new() };
        *y = top;
        if pages.is_empty() && !title.is_empty() {
            p.text(MARGIN, *y - 12.0, 12.0, true, title);
            *y -= 24.0;
        }
        for (c, h) in headers.iter().enumerate() {
            p.text(
                col_x[c],
                *y - fs,
                fs,
                true,
                &h.chars().take(widths[c]).collect::<String>(),
            );
        }
        *y -= lh + 2.0;
        p.line(MARGIN, *y, right, *y, 0.8);
        *y -= 3.0;
        pages.push(p);
    };
    new_page(&mut pages, &mut y);
    for row in rows {
        let cells: Vec<Vec<String>> = row
            .iter()
            .enumerate()
            .map(|(c, s)| wrap(s, widths[c]))
            .collect();
        let n = cells.iter().map(Vec::len).max().unwrap_or(1);
        if y - n as f32 * lh < bottom {
            new_page(&mut pages, &mut y);
        }
        let p = pages.last_mut().unwrap();
        for (c, lines) in cells.iter().enumerate() {
            for (i, l) in lines.iter().enumerate() {
                p.text(col_x[c], y - fs - i as f32 * lh, fs, false, l);
            }
        }
        y -= n as f32 * lh + 2.0;
        p.line(MARGIN, y, right, y, 0.25);
        y -= 2.0;
    }
    pages
}

fn layout_cards(
    headers: &[String],
    rows: &[Vec<String>],
    title: &str,
    (pw, ph): (f32, f32),
) -> Vec<Page> {
    let fs = 9.0;
    let lh = fs * 1.3;
    let pad = 6.0;
    let box_w = (pw - 2.0 * MARGIN).min(400.0);
    let label_chars = headers
        .iter()
        .map(|h| h.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(4, 20);
    let value_x = MARGIN + pad + (label_chars + 2) as f32 * CHAR_W * fs;
    let value_chars = ((MARGIN + box_w - pad - value_x) / (CHAR_W * fs)) as usize;
    let top = ph - MARGIN;
    let bottom = MARGIN + 14.0;

    let mut pages: Vec<Page> = vec![];
    let mut y = 0.0;
    let new_page = |pages: &mut Vec<Page>, y: &mut f32| {
        let mut p = Page { ops: String::new() };
        *y = top;
        if pages.is_empty() && !title.is_empty() {
            p.text(MARGIN, *y - 12.0, 12.0, true, title);
            *y -= 24.0;
        }
        pages.push(p);
    };
    new_page(&mut pages, &mut y);
    for row in rows {
        let cells: Vec<Vec<String>> = row.iter().map(|s| wrap(s, value_chars)).collect();
        let n: usize = cells.iter().map(Vec::len).sum();
        let h = n as f32 * lh + 2.0 * pad;
        if y - h < bottom && y < top {
            new_page(&mut pages, &mut y);
        }
        let p = pages.last_mut().unwrap();
        p.rect(MARGIN, y - h, box_w, h, 0.6);
        let mut ly = y - pad;
        for (c, lines) in cells.iter().enumerate() {
            p.text(
                MARGIN + pad,
                ly - fs,
                fs,
                true,
                &headers[c].chars().take(label_chars).collect::<String>(),
            );
            for l in lines {
                p.text(value_x, ly - fs, fs, false, l);
                ly -= lh;
            }
        }
        y -= h + 12.0;
    }
    pages
}

/// Render `rows` (cells of the columns named in `headers`) to a PDF document.
pub fn render_pdf(headers: &[String], rows: &[Vec<String>], opts: &PrintOpts) -> Vec<u8> {
    let size = page_size(opts.landscape);
    let pages = match opts.layout {
        Layout::Table => layout_table(headers, rows, &opts.title, size),
        Layout::Cards => layout_cards(headers, rows, &opts.title, size),
    };
    assemble(pages, size)
}

/// Strings for one card of a table with a schema `card` (see [`card_rows`]).
pub struct CardRow {
    /// Value of the card's `height_field`.
    pub height_key: Option<String>,
    /// Cell values in the order of `card.cells` flattened.
    pub cells: Vec<String>,
    pub text: String,
}

/// Formatted card contents for `ids`. `None` when the table has no `card`.
pub fn card_rows(store: &Store, table: usize, ids: &[u64]) -> Option<Vec<CardRow>> {
    let t = &store.schema.tables[table];
    let card = t.card.as_ref()?;
    let fmt = |r: &latent_data::Row, name: &str| {
        t.field_index(name)
            .map(|i| format_value(&r.values[i], &t.fields[i]))
            .unwrap_or_default()
    };
    let by_id: std::collections::HashMap<u64, &latent_data::Row> =
        store.tables[table].rows.iter().map(|r| (r.id, r)).collect();
    Some(
        ids.iter()
            .filter_map(|id| by_id.get(id))
            .map(|r| CardRow {
                height_key: card.height_field.as_deref().map(|f| fmt(r, f)),
                cells: card
                    .cells
                    .iter()
                    .flatten()
                    .map(|c| fmt(r, &c.field))
                    .collect(),
                text: card
                    .text
                    .as_ref()
                    .map(|x| fmt(r, &x.field))
                    .unwrap_or_default(),
            })
            .collect(),
    )
}

const MM: f32 = 72.0 / 25.4;
const CARD_FS: f32 = 8.0;

/// Lay out cards exactly as the schema `card` describes (mm on A4 portrait, as the Qt app did).
pub fn render_cards_pdf(card: &latent_schema::Card, rows: &[CardRow]) -> Vec<u8> {
    let page_h = A4.1 / MM;
    let char_mm = CARD_FS * CHAR_W / MM;
    let line_mm = CARD_FS * 1.25 / MM;
    let (x0, w) = (card.x, card.width);
    let flat: Vec<&latent_schema::CardCell> = card.cells.iter().flatten().collect();
    let grid_h = 5.0 * card.cells.len() as f32;
    let val_at = |c: &latent_schema::CardCell| c.value_at.unwrap_or(card.label_width);
    // Inner vertical dividers over the cell grid (the label divider spans the whole card).
    let mut dividers: Vec<f32> = vec![];
    for c in &flat {
        for v in [c.at.unwrap_or(0.0), val_at(c)] {
            if v > 0.0 && v != card.label_width && !dividers.contains(&v) {
                dividers.push(v);
            }
        }
    }

    let new_page = |pages: &mut Vec<Page>| pages.push(Page { ops: String::new() });
    let mut pages = vec![];
    new_page(&mut pages);
    let mut y = card.y;
    for row in rows {
        let h = card.height_for(row.height_key.as_deref());
        if y > page_h - card.bottom_margin - h {
            new_page(&mut pages);
            y = card.bottom_margin;
        }
        let p = pages.last_mut().unwrap();
        let px = |x: f32, y: f32| (x * MM, (page_h - y) * MM);
        let mut k = 0;
        for (ri, line) in card.cells.iter().enumerate() {
            for (i, c) in line.iter().enumerate() {
                let row_y = y + 5.0 * ri as f32 + 4.0;
                let label = format!("{}:", c.label.as_deref().unwrap_or(&c.field));
                let (lx, ly) = px(x0 + c.at.unwrap_or(0.0) + 1.0, row_y);
                p.text(lx, ly, CARD_FS, true, &label);
                // The value may not run into the next cell's label (or the card edge).
                let end = line.get(i + 1).map_or(w, |n| n.at.unwrap_or(0.0));
                let fit = (((end - val_at(c) - 1.0) / char_mm) as usize).max(1);
                let (vx, vy) = px(x0 + val_at(c) + 1.0, row_y);
                p.text(
                    vx,
                    vy,
                    CARD_FS,
                    false,
                    &row.cells[k].chars().take(fit).collect::<String>(),
                );
                k += 1;
            }
        }
        if let Some(t) = &card.text {
            let base = grid_h + 4.0;
            let (lx, ly) = px(x0 + 1.0, y + base);
            p.text(
                lx,
                ly,
                CARD_FS,
                true,
                &format!("{}:", t.label.as_deref().unwrap_or(&t.field)),
            );
            let chars = ((w - card.label_width - 2.0) / char_mm) as usize;
            for (n, l) in wrap(&row.text, chars).iter().enumerate() {
                let by = base + n as f32 * line_mm;
                if by > h - 1.0 {
                    break;
                }
                let (vx, vy) = px(x0 + card.label_width + 1.0, y + by);
                p.text(vx, vy, CARD_FS, false, l);
            }
        }
        // Rules: inner lines first, then the outer rectangle (Qt pen widths 0.35 and 0.6).
        let rule = |p: &mut Page, ax: f32, ay: f32, bx: f32, by: f32| {
            let (a, b) = (px(ax, ay), px(bx, by));
            p.line(a.0, a.1, b.0, b.1, 0.35 * MM);
        };
        for i in 1..=card.cells.len() {
            rule(p, x0, y + 5.0 * i as f32, x0 + w, y + 5.0 * i as f32);
        }
        for d in &dividers {
            rule(p, x0 + d, y, x0 + d, y + grid_h);
        }
        rule(p, x0 + card.label_width, y, x0 + card.label_width, y + h);
        let (rx, ry) = px(x0, y + h);
        p.rect(rx, ry, w * MM, h * MM, 0.6 * MM);
        y += h;
    }
    assemble(pages, A4)
}

/// Add page numbers and write the PDF document.
fn assemble(mut pages: Vec<Page>, size: (f32, f32)) -> Vec<u8> {
    let count = pages.len();
    for (i, p) in pages.iter_mut().enumerate() {
        p.text(
            size.0 - MARGIN - 8.0 * 9.0 * CHAR_W,
            MARGIN - 4.0,
            9.0,
            false,
            &format!("Seite {}/{}", i + 1, count),
        );
    }

    // Objects: 1 catalog, 2 pages, 3 Courier, 4 Courier-Bold, then (page, content) pairs.
    let mut objs: Vec<String> = vec![String::new(); 4];
    objs[0] = "<< /Type /Catalog /Pages 2 0 R >>".into();
    let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    objs[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {count} >>",
        kids.join(" ")
    );
    objs[2] =
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>".into();
    objs[3] =
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier-Bold /Encoding /WinAnsiEncoding >>"
            .into();
    for (i, p) in pages.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
            size.0,
            size.1,
            6 + 2 * i
        ));
        objs.push(format!(
            "<< /Length {} >>\nstream\n{}endstream",
            p.ops.len(),
            p.ops
        ));
    }

    // Everything is ASCII (non-ASCII text is octal-escaped), so str offsets are byte offsets.
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = vec![];
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objs.len() + 1
    ));
    for off in offsets {
        out.push_str(&format!("{off:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    ));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(layout: Layout) -> PrintOpts {
        PrintOpts {
            layout,
            landscape: false,
            title: "Rollfilme".into(),
            columns: vec![],
            selected_only: false,
        }
    }

    fn sample(n: usize) -> (Vec<String>, Vec<Vec<String>>) {
        let headers = vec!["Nr".to_string(), "Motiv (Größe)".to_string()];
        let rows = (0..n)
            .map(|i| {
                vec![
                    i.to_string(),
                    format!("Bäume (a\\b) {}", "lang ".repeat(30)),
                ]
            })
            .collect();
        (headers, rows)
    }

    #[test]
    fn pdf_header_and_trailer() {
        let (h, r) = sample(3);
        for layout in [Layout::Table, Layout::Cards] {
            let pdf = render_pdf(&h, &r, &opts(layout));
            assert!(pdf.starts_with(b"%PDF-"));
            assert!(pdf.ends_with(b"%%EOF\n"));
        }
    }

    #[test]
    fn many_rows_paginate() {
        let (h, r) = sample(200);
        for layout in [Layout::Table, Layout::Cards] {
            let pdf = String::from_utf8(render_pdf(&h, &r, &opts(layout))).unwrap();
            let pages = pdf.matches("/Type /Page ").count();
            assert!(pages > 1, "{layout:?} gave {pages} page(s)");
        }
    }

    #[test]
    fn empty_input_gives_one_page() {
        let pdf = String::from_utf8(render_pdf(&["A".into()], &[], &opts(Layout::Table))).unwrap();
        assert_eq!(pdf.matches("/Type /Page ").count(), 1);
    }

    #[test]
    fn escapes_and_encodes() {
        assert_eq!(pdf_escape("a(b)\\"), "a\\(b\\)\\\\");
        assert_eq!(pdf_escape("ä"), "\\344");
        assert_eq!(pdf_escape("日"), "?");
    }

    #[test]
    fn wrap_breaks_words_and_long_runs() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap("a\nb", 5), vec!["a", "b"]);
    }

    fn qt_card() -> latent_schema::Card {
        let s =
            latent_schema::Schema::from_json(include_str!("../../../schemas/latent.json")).unwrap();
        s.table("roll_film").unwrap().card.clone().unwrap()
    }

    fn film(format: &str) -> CardRow {
        let cells = [
            "12",
            "400",
            "01.02.2024",
            "Rodinal",
            "HP5",
            "08:30",
            "Leica",
            "20.0",
        ];
        CardRow {
            height_key: Some(format.into()),
            cells: cells.map(String::from).to_vec(),
            text: "Bäume im Park".into(),
        }
    }

    #[test]
    fn cards_use_qt_geometry() {
        let card = qt_card();
        let pdf =
            String::from_utf8(render_cards_pdf(&card, &[film("135er"), film("120er")])).unwrap();
        let mm = |v: f32| format!("{:.2}", v * MM);
        // 40 mm from the left, 130 mm wide; first box 36 mm tall starting 20 mm from the top.
        assert!(
            pdf.contains(&format!(
                "{} {} {} {} re S",
                mm(40.0),
                mm(297.0 - 20.0 - 36.0),
                mm(130.0),
                mm(36.0)
            )),
            "{pdf}"
        );
        // The 120er box follows directly below (no gap), 58 mm tall.
        assert!(pdf.contains(&format!(
            "{} {} {} {} re S",
            mm(40.0),
            mm(297.0 - 56.0 - 58.0),
            mm(130.0),
            mm(58.0)
        )));
        // Inner dividers at x+65 and x+95, 20 mm tall.
        assert!(pdf.contains(&format!(
            "{} {} m {} {} l S",
            mm(105.0),
            mm(297.0 - 20.0),
            mm(105.0),
            mm(297.0 - 40.0)
        )));
        assert!(pdf.contains(&format!(
            "{} {} m {} {} l S",
            mm(135.0),
            mm(297.0 - 20.0),
            mm(135.0),
            mm(297.0 - 40.0)
        )));
        assert!(
            pdf.contains("(Film Nr.:)")
                && pdf.contains("(Temperatur:)")
                && pdf.contains("(Motiv:)")
        );
        assert!(pdf.contains("(B\\344ume im Park)"));
    }

    #[test]
    fn cards_paginate_like_qt() {
        let card = qt_card();
        let count = |n: usize| {
            let rows: Vec<CardRow> = (0..n).map(|_| film("120er")).collect();
            String::from_utf8(render_cards_pdf(&card, &rows))
                .unwrap()
                .matches("/Type /Page ")
                .count()
        };
        // The 4th box starts at 20 + 3*58 = 194 <= 297 - 20 - 58 = 219, the 5th at 252 > 219 breaks the page.
        assert_eq!(count(4), 1);
        assert_eq!(count(5), 2);
        assert_eq!(count(0), 1);
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        let (h, r) = sample(2);
        let pdf = String::from_utf8(render_pdf(&h, &r, &opts(Layout::Table))).unwrap();
        let xref = pdf.find("xref\n").unwrap();
        let first: usize = pdf[xref..].lines().nth(3).unwrap()[..10].parse().unwrap();
        assert!(pdf[first..].starts_with("1 0 obj"));
    }
}
