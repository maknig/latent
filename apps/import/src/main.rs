//! One-off importer for the old Qt FilmArchiv data folder (`~/.filmarchiv`).
//!
//! Usage: fa-import <source> <target> [--apply] [--force]
//! Without `--apply` nothing is written (dry run). The source is only read.
use fa_data::{fs, image_paths, parse_value, Store, TableData, Value};
use fa_schema::{Schema, Table};
use std::path::{Path, PathBuf};

const IMAGE_EXTS: [&str; 5] = ["jpg", "jpeg", "png", "tif", "tiff"];

fn is_image(p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    !name.starts_with("._")
        && p.extension().is_some_and(|e| IMAGE_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Compare file names so `Bild_2` sorts before `Bild_10`.
fn natural_key(s: &str) -> Vec<(u8, String, u64)> {
    let mut out = vec![];
    let lower = s.to_lowercase();
    let mut chars = lower.chars().peekable();
    while let Some(&c) = chars.peek() {
        let digit = c.is_ascii_digit();
        let mut run = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() != digit {
                break;
            }
            run.push(c);
            chars.next();
        }
        out.push(if digit { (0, String::new(), run.parse().unwrap_or(0)) } else { (1, run, 0) });
    }
    out
}

fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_image(p)).collect())
        .unwrap_or_default();
    v.sort_by_key(|p| natural_key(&p.file_name().unwrap_or_default().to_string_lossy()));
    v
}

/// Read a text file as UTF-8 (lossy) with CRLF normalised.
fn read_text(p: &Path) -> Result<String, String> {
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(String::from_utf8_lossy(&bytes).replace("\r\n", "\n"))
}

/// CSV with a header row (the old app wrote one for film tables).
fn import_csv_with_header(table: &Table, path: &Path, notes: &mut Vec<String>) -> Result<TableData, String> {
    let text = read_text(path)?;
    let (header, rest) = text.split_once('\n').unwrap_or((&text, ""));
    if !header.trim_start_matches('\u{feff}').to_lowercase().starts_with("id,") {
        return Err(format!("{}: unexpected header '{header}'", path.display()));
    }
    notes.push(format!("{}: header '{header}'", path.display()));
    TableData::from_csv(table, rest).map_err(|e| format!("{}: {e} (row numbers exclude the header)", path.display()))
}

/// Per-image `.txt`: first line is the tag list, following lines are enlargement rows
/// (date, enlarger, lens, exposure, height, paper, developer, develop time, notice).
/// A first line that already looks like an enlargement row is treated as one.
/// Returns (rows, tags, notes).
fn parse_enlargement_txt(table: &Table, text: &str) -> (Vec<Vec<Value>>, Option<String>, Vec<String>) {
    let mut notes = vec![];
    let mut tags = None;
    let text = text.replace("\r\n", "\n");
    let mut rd = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(text.as_bytes());
    let col = |n: &str| table.field_index(n);
    let order = ["date", "enlarger", "lens", "exposure_time", "enlarger_height", "paper", "developer", "develop_time", "notice"];
    let mut rows = vec![];
    let mut dateless = 0;
    for (n, rec) in rd.records().enumerate() {
        let Ok(rec) = rec else {
            notes.push(format!("line {}: unreadable, skipped", n + 1));
            continue;
        };
        if rec.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let mut values = TableData::blank_values(table);
        for v in values.iter_mut() {
            *v = Value::Null; // no defaults for imported data
        }
        let date_ok = table
            .field_index("date")
            .and_then(|i| parse_value(rec.get(0).unwrap_or(""), &table.fields[i]).ok())
            .is_some_and(|v| v != Value::Null);
        let mut ok = date_ok;
        if ok {
            for (i, name) in order.iter().enumerate() {
                let (Some(c), Some(cell)) = (col(name), rec.get(i)) else { continue };
                match parse_value(cell, &table.fields[c]) {
                    Ok(v) => values[c] = v,
                    Err(e) => {
                        notes.push(format!("line {}: {e}", n + 1));
                        ok = false;
                        break;
                    }
                }
            }
        }
        if ok {
            rows.push(values);
        } else if n == 0 {
            let t = rec.iter().map(str::trim).filter(|c| !c.is_empty()).collect::<Vec<_>>().join(", ");
            if !t.is_empty() {
                tags = Some(t);
            }
        } else {
            dateless += 1;
        }
    }
    if dateless > 0 {
        notes.push(format!("{dateless} line(s) without a valid date in the first column skipped"));
    }
    (rows, tags, notes)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = |f: &str| args.iter().any(|a| a == f);
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if paths.len() != 2 {
        eprintln!("usage: fa-import <source> <target> [--apply] [--force]");
        std::process::exit(2);
    }
    let (src, dst) = (PathBuf::from(paths[0]), PathBuf::from(paths[1]));
    let (apply, force) = (flags("--apply"), flags("--force"));
    if let Err(e) = run(&src, &dst, apply, force) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(src: &Path, dst: &Path, apply: bool, force: bool) -> Result<(), String> {
    println!("{} {} -> {}", if apply { "IMPORT" } else { "DRY RUN" }, src.display(), dst.display());
    let schema = Schema::from_json(include_str!("../../../schemas/filmarchiv.json")).map_err(|e| e.to_string())?;
    let tix = |n: &str| schema.tables.iter().position(|t| t.name == n).ok_or(format!("schema lacks table {n}"));
    let (ri, pi, ei) = (tix("roll_film")?, tix("plan_film")?, tix("enlargement")?);
    let mut store = Store::new(schema.clone());
    let mut notes = vec![];

    if apply && !force && (dst.join("filmarchiv.data.json").exists() || dst.join("images").read_dir().is_ok_and(|mut d| d.next().is_some())) {
        return Err(format!("{} is not empty; pass --force to write into it", dst.display()));
    }

    store.tables[ri] = import_csv_with_header(&schema.tables[ri], &src.join("archiv.csv"), &mut notes)?;
    store.tables[pi] = import_csv_with_header(&schema.tables[pi], &src.join("planFilmArchiv.csv"), &mut notes)?;
    println!("roll_film:  {} rows", store.tables[ri].rows.len());
    println!("plan_film:  {} rows (their image folders are not imported)", store.tables[pi].rows.len());

    let img_field = |t: &Table| t.field_index("images").ok_or(format!("table {} lacks an images field", t.name));
    let (roll_img, enl_img) = (img_field(&schema.tables[ri])?, img_field(&schema.tables[ei])?);
    let enl_src = schema.tables[ei].image_ref_index().ok_or("enlargement table lacks an image_ref field")?;
    let info_idx = schema.role_index(fa_schema::TableRole::ImageInfo);

    let mut film_dirs: Vec<(u64, PathBuf)> = std::fs::read_dir(src.join("images"))
        .map_err(|e| format!("images: {e}"))?
        .flatten()
        .filter_map(|e| Some((e.file_name().to_string_lossy().strip_prefix("film_")?.parse().ok()?, e.path())))
        .collect();
    film_dirs.sort();

    let (mut n_img, mut bytes, mut n_enl, mut n_tags) = (0usize, 0u64, 0usize, 0usize);
    for (id, dir) in &film_dirs {
        if !store.tables[ri].rows.iter().any(|r| r.id == *id) {
            notes.push(format!("{}: no roll film with id {id}, folder skipped", dir.display()));
            continue;
        }
        let files = list_images(dir);
        let mut rels = vec![];
        for f in &files {
            bytes += std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            if apply {
                rels.push(fs::import_image(dst, "roll_film", *id, f).map_err(|e| format!("{}: {e}", f.display()))?);
            } else {
                rels.push(format!("images/roll_film/{id:04}/{}", f.file_name().unwrap_or_default().to_string_lossy()));
            }
        }
        n_img += files.len();
        if let Some(r) = store.tables[ri].get_mut(*id) {
            r.values[roll_img] = Value::Text(rels.join("\n"));
        }

        // Enlargement data files next to the images.
        let mut txts: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt") && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("._"))).collect())
            .unwrap_or_default();
        txts.sort();
        for txt in txts {
            let stem = txt.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            // `x.png.txt` names its image exactly, `x.txt` matches by stem.
            let image = files.iter().find(|f| f.file_name().is_some_and(|n| n.to_string_lossy() == stem)).or_else(|| {
                files.iter().find(|f| f.file_stem().is_some_and(|s| s.to_string_lossy() == stem))
            });
            let (rows, tags, tnotes) = parse_enlargement_txt(&schema.tables[ei], &read_text(&txt)?);
            let label = txt.strip_prefix(src).unwrap_or(&txt).display().to_string();
            for n in tnotes {
                notes.push(format!("{label}: {n}"));
            }
            let Some(image) = image else {
                notes.push(format!("{label}: no matching image, {} row(s) skipped", rows.len()));
                continue;
            };
            if let (Some(tags), Some(n), Some(ii)) = (tags, files.iter().position(|f| f == image), info_idx) {
                let t = &schema.tables[ii];
                let mut values = TableData::blank_values(t);
                values[t.image_ref_index().unwrap_or(0)] = Value::Text(rels[n].clone());
                if let Some(c) = t.field_index("tags") {
                    values[c] = Value::Text(tags);
                }
                store.tables[ii].insert(t, values);
                n_tags += 1;
            }
            for mut values in rows {
                let id = store.tables[ei].next_id;
                let rel = if apply {
                    fs::import_image(dst, "enlargement", id, image).map_err(|e| e.to_string())?
                } else {
                    format!("images/enlargement/{id:04}/{}", image.file_name().unwrap_or_default().to_string_lossy())
                };
                values[enl_img] = Value::Text(rel);
                if let Some(n) = files.iter().position(|f| f == image) {
                    values[enl_src] = Value::Text(rels[n].clone());
                }
                store.tables[ei].insert(&schema.tables[ei], values);
                n_enl += 1;
                println!("enlargement #{id}: {label} -> {}", image.file_name().unwrap_or_default().to_string_lossy());
            }
        }
    }
    println!("roll film images: {n_img} files, {:.1} MB in {} folders", bytes as f64 / 1e6, film_dirs.len());
    println!("enlargements: {n_enl} rows");
    println!("image tags: {n_tags} images");
    for n in &notes {
        println!("note: {n}");
    }

    if !apply {
        println!("dry run only, nothing written. Re-run with --apply to import.");
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    fs::save(&store, &dst.join("filmarchiv.schema.json"), &dst.join("filmarchiv.data.json")).map_err(|e| e.to_string())?;

    // Verify: reload and check that every referenced image exists.
    let (back, warnings) = fs::load(&dst.join("filmarchiv.schema.json"), &dst.join("filmarchiv.data.json")).map_err(|e| e.to_string())?;
    let mut missing = 0;
    let mut refs = 0;
    for (t, td) in back.schema.tables.iter().zip(&back.tables) {
        if let Some(fi) = t.field_index("images") {
            for r in &td.rows {
                for p in image_paths(&r.values[fi]) {
                    refs += 1;
                    if !dst.join(&p).exists() {
                        missing += 1;
                        println!("MISSING {p}");
                    }
                }
            }
        }
    }
    println!("verify: {} warnings, {refs} image references, {missing} missing", warnings.len());
    for w in warnings {
        println!("warning: {w}");
    }
    if back != store || missing > 0 {
        return Err("verification failed".into());
    }
    println!("done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enl() -> Table {
        let s = Schema::from_json(include_str!("../../../schemas/filmarchiv.json")).unwrap();
        s.table("enlargement").unwrap().clone()
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["Bild_10.png", "Bild_2.png", "Bild_1.png"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, vec!["Bild_1.png", "Bild_2.png", "Bild_10.png"]);
    }

    #[test]
    fn empty_tag_line_then_rows() {
        let t = enl();
        let txt = "\r\n25.12.2021,Leitz,Apo 80,01:30,33,Paper,Dev,02:00,note\r\n25.12.2021,B,L,00:00,,,,00:00,\r\n";
        let (rows, tags, notes) = parse_enlargement_txt(&t, txt);
        assert_eq!(tags, None);
        assert_eq!(rows.len(), 2, "{notes:?}");
        assert_eq!(rows[0][t.field_index("exposure_time").unwrap()], Value::Time(90));
        assert_eq!(rows[0][t.field_index("notice").unwrap()], Value::Text("note".into()));
        assert_eq!(rows[0][t.field_index("aperture").unwrap()], Value::Null);
    }

    #[test]
    fn first_line_enlargement_without_tags() {
        let (rows, _, _) = parse_enlargement_txt(&enl(), "21.11.2020,JOBO,Lens,00:00,33,Paper,Dev,00:00,\n");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn tag_line_is_kept_and_dateless_rows_skipped() {
        let (rows, tags, notes) = parse_enlargement_txt(&enl(), "cat,mountain\n,21.07.2019,Kodak,,Mamiya\n");
        assert!(rows.is_empty());
        assert_eq!(tags.as_deref(), Some("cat, mountain"));
        assert!(notes.iter().any(|n| n.contains("without a valid date")), "{notes:?}");
    }

    #[test]
    fn header_csv_import() {
        let s = Schema::from_json(include_str!("../../../schemas/filmarchiv.json")).unwrap();
        let t = s.table("roll_film").unwrap();
        let dir = std::env::temp_dir().join("fa_import_hdr");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.csv");
        std::fs::write(&p, "ID,Datum,Film\r\n7,05.02.2006,Agfa,135er,Nikon,100,Dev,08:30,20,\"A,B\"\r\n").unwrap();
        let td = import_csv_with_header(t, &p, &mut vec![]).unwrap();
        assert_eq!(td.rows.len(), 1);
        assert_eq!(td.rows[0].id, 7);
        std::fs::write(&p, "7,05.02.2006,Agfa\r\n").unwrap();
        assert!(import_csv_with_header(t, &p, &mut vec![]).is_err());
    }
}
