use std::path::PathBuf;

/// Archive folder: first CLI argument, else `<Documents>/FilmArchiv` (home if no Documents).
fn archive_dir() -> PathBuf {
    if let Some(a) = std::env::args().nth(1) {
        return PathBuf::from(a);
    }
    match dirs::document_dir() {
        Some(d) => d.join("FilmArchiv"),
        None => {
            eprintln!("no Documents folder found, using the home directory");
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("FilmArchiv")
        }
    }
}

fn main() -> eframe::Result {
    let dir = archive_dir();
    if let Err(e) = std::fs::create_dir_all(dir.join("images")) {
        eprintln!("cannot create {}: {e}", dir.display());
        std::process::exit(1);
    }
    let schema_path = dir.join("filmarchiv.schema.json");
    let data_path = dir.join("filmarchiv.data.json");

    if !schema_path.exists() {
        if let Err(e) = std::fs::write(&schema_path, include_str!("../../../schemas/filmarchiv.json")) {
            eprintln!("cannot write schema: {e}");
            std::process::exit(1);
        }
    }
    let store = if data_path.exists() {
        match fa_data::fs::load(&schema_path, &data_path) {
            Ok((s, w)) => {
                for m in w {
                    eprintln!("warning: {m}");
                }
                s
            }
            Err(e) => {
                eprintln!("cannot load: {e}");
                std::process::exit(1)
            }
        }
    } else {
        let json = std::fs::read_to_string(&schema_path).unwrap_or_default();
        match fa_schema::Schema::from_json(&json) {
            Ok(s) => fa_data::Store::new(s),
            Err(e) => {
                eprintln!("invalid schema {}: {e}", schema_path.display());
                std::process::exit(1)
            }
        }
    };

    let mut app = fa_ui::App::new(store);
    app.base_dir = dir;
    app.on_save = Some(Box::new(move |s| {
        fa_data::fs::save(s, &schema_path, &data_path).map_err(|e| e.to_string())
    }));
    eframe::run_native("FilmArchiv", eframe::NativeOptions::default(), Box::new(|_| Ok(Box::new(app))))
}
