use std::path::PathBuf;

/// Archive folder: first CLI argument, else `<Documents>/Latent` (home if no Documents).
fn archive_dir() -> PathBuf {
    if let Some(a) = std::env::args().nth(1) {
        return PathBuf::from(a);
    }
    match dirs::document_dir() {
        Some(d) => d.join("Latent"),
        None => {
            eprintln!("no Documents folder found, using the home directory");
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Latent")
        }
    }
}

fn main() -> eframe::Result {
    let dir = archive_dir();
    if let Err(e) = std::fs::create_dir_all(dir.join("images")) {
        eprintln!("cannot create {}: {e}", dir.display());
        std::process::exit(1);
    }
    let schema_path = dir.join("latent.schema.json");
    let data_path = dir.join("latent.data.json");

    if !schema_path.exists() {
        if let Err(e) = std::fs::write(&schema_path, include_str!("../../../schemas/latent.json")) {
            eprintln!("cannot write schema: {e}");
            std::process::exit(1);
        }
    }
    let mut upgraded = false;
    let store = if data_path.exists() {
        match latent_data::fs::load(&schema_path, &data_path) {
            Ok((s, w)) => {
                for m in w {
                    upgraded |= m == latent_data::UPGRADE_WARNING;
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
        match latent_schema::Schema::from_json(&json) {
            Ok(s) => latent_data::Store::new(s),
            Err(e) => {
                eprintln!("invalid schema {}: {e}", schema_path.display());
                std::process::exit(1)
            }
        }
    };

    let mut app = latent_ui::App::new(store);
    app.engine.dirty = upgraded;
    app.base_dir = dir;
    app.on_save = Some(Box::new(move |s| {
        latent_data::fs::save(s, &schema_path, &data_path).map_err(|e| e.to_string())
    }));
    eframe::run_native(
        "Latent",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(app))),
    )
}
