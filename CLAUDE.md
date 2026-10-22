# CLAUDE.md

Rust port of the Qt/C++ analog-film archive app in `../filmarchiv`. It is schema-driven, has no database, and the UI is egui (eframe/egui 0.31) with a native and a wasm build. UI labels are German (Rollfilme, Datum, Kamera).

## Layout

Cargo workspace (resolver 2):

- `crates/schema` (`fa-schema`): `Schema`, `Table` (optional `role`: `TableRole::Film` / `Enlargement`), `Field`, `FieldType` (Id, Int, Float, Bool, Date, Time, Text, Enum, Images, ImageRef). JSON in/out and validation.
- `crates/data` (`fa-data`): `Value`, `Row`, `TableData`, `Store`, CSV/JSON I/O. The `fs` module has atomic save, `import_image` and unused-image listing.
- `crates/engine` (`fa-engine`): `Engine` (insert/update/delete, undo/redo) and `view(table, &Query)`. `Query` has sort, search, column filters, enum equals and ranges.
- `crates/ui` (`fa-ui`): the egui `App` (`lib.rs`), thumbnails (`images.rs`) and the dark theme (`apply_theme`).
- `crates/vision` (`fa-vision`): native-only ONNX models (`ort`, `Model` trait, `OnnxModel`), cosine/HDBSCAN, embedding cache. Models live in `<archive>/models/<name>.onnx` + `<name>.json` (`size`, optional `mean`, `std`, `output`); embeddings are cached in `filmarchiv.vision.json`. UI: image strip context menu "Find similar images" (`crates/ui/src/similar.rs`). Image view "Auto tags" (`crates/ui/src/tagger.rs`) merges tags of similar tagged images with CLIP zero-shot scores of the archive's tags; CLIP needs `clip-image.onnx`+`.json` (`"kind":"clip-image"`), `clip-text.onnx`+`.json` (`"kind":"clip-text"`) and `tokenizer.json` in `models/`, optional `tag_prompts.json` (tag -> English prompt). Models with a `kind` are skipped by the plain embedding lookup.
- `apps/desktop` (package `filmarchiv`): native eframe app.
- `apps/web` (`filmarchiv-web`): wasm build via trunk (`dist` is gitignored).
- `apps/import` (`fa-import`): one-off importer from the old Qt data.
- `schemas/filmarchiv.json`: default schema (tables `roll_film`, `plan_film`, `enlargement`), embedded with `include_str!` in the desktop app and the importer.
- `tests/fixtures/`: CSV fixtures used by the data/engine tests.

## Commands

There is no CI, justfile or Makefile.

```
nix develop                          # dev shell (rust stable + wasm32, trunk, GL/wayland/X11 libs)
cargo run -p filmarchiv [archive_dir]
cargo test --workspace
cargo clippy --workspace
cargo fmt
cd apps/web && trunk serve
cargo run -p fa-import -- <src> <dst> [--apply] [--force]
```

## Architecture

- **Archive folder** (`apps/desktop/src/main.rs`): first CLI arg, else `<Documents>/FilmArchiv`, else `~/FilmArchiv`. It contains `filmarchiv.schema.json` (copied from the default if missing), `filmarchiv.data.json` and `images/`.
- **Name-based storage**: data is loaded and saved by field name, so reordering fields is safe. Cells that no longer match the schema are kept as orphans and reported as warnings. They can be purged.
- **Saves** are atomic (temp file + rename) and keep a `.bak`.
- **Ids**: a table with an `id` field stores it. Tables without one use a persistent `_id` plus a per-table `next_id`.
- **Images**: an `images` field holds newline-separated paths relative to the archive folder. `import_image` copies files to `images/<table>/<id:04>/`, reuses identical files and suffixes name clashes with `-n`. The app can list and delete unused images.
- **Image links**: the table with `"role": "enlargement"` has exactly one `image_ref` field holding the archive-relative path of a film image (film tables have `"role": "film"`). One image can have many enlargements. `fa_data::enlargements_for` / `film_row_for` resolve links; the film image strip offers "New enlargement" / "Show enlargements", the enlargement form has a picker. Old schemas (default table names, no roles) are upgraded on load by `Schema::upgrade_links`; the upgrade is kept on the next save.
- **Printing** (`crates/ui/src/print.rs`, File > Print): writes a PDF by hand (Courier, no dependency). "Table" lists the chosen columns; "Cards" prints a table's schema `card` (optional `Table.card`: mm geometry, `height_field` + `heights`, `cells` rows of label/field pairs, free `text`) in the Qt sheet layout, or generic cards if the table has none. The default schema carries the Qt values for `roll_film` and `plan_film`; archives with an older schema copy need the `card` block added by hand.
- **Times** are stored as seconds (`u32`), formatted `%H:%M` or `%M:%S`.
- **App wiring**: `App::new(store)` takes `base_dir` (root for image paths). `on_save` is the save hook. `persist` selects eframe storage (localStorage on web) for autosave.

## Relation to the Qt original

- Old data lives in `~/.filmarchiv`: `archiv.csv` (roll film), `planFilmArchiv.csv`, `images/<film id>/`, and per-image `.txt` files (first line tags, then enlargement rows: date, enlarger, lens, exposure, height, paper, developer, develop time, notice). `x.png.txt` matches an image by full name, `x.txt` by stem.
- `fa-import` migrates it. It is a dry run unless `--apply`, and refuses to overwrite existing data without `--force`.
- Not ported as far as known (unverified): image clustering (`ImageCluster.cpp`, `hdbscan/`) and the dedicated enlargement dialog. The generic table/form UI replaces the per-widget Qt forms.

## Conventions

- rustfmt defaults; run `cargo fmt` before committing.
- Libraries use `thiserror` enums (`DataError`, `SchemaError`). Binaries use `String` errors with `eprintln!` and `process::exit`.
- Tests are inline `#[cfg(test)]` modules in `data` and `engine`.
- Commit messages are short English imperative sentences.
- `filmarchiv.schema.json`, `filmarchiv.data.json` and `*.bak` in the repo root are gitignored sample data.
