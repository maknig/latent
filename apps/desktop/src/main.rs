#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;

/// Reports a startup error on stderr and in a dialog (no terminal when started from Finder or
/// Explorer), then exits.
fn fail(msg: String) -> ! {
    eprintln!("{msg}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Latent")
        .set_description(&msg)
        .show();
    std::process::exit(1)
}

/// Archive folder: first CLI argument, else `<Documents>/Latent` (home if no Documents).
fn archive_dir() -> PathBuf {
    // Older macOS passes `-psn_<process serial number>` when started from Finder.
    if let Some(a) = std::env::args().nth(1).filter(|a| !a.starts_with("-psn_")) {
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
        fail(format!("cannot create {}: {e}", dir.display()));
    }
    let schema_path = dir.join("latent.schema.json");
    let data_path = dir.join("latent.data.json");

    if !schema_path.exists() {
        if let Err(e) = std::fs::write(&schema_path, include_str!("../../../schemas/latent.json")) {
            fail(format!("cannot write schema: {e}"));
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
            Err(e) => fail(format!("cannot load: {e}")),
        }
    } else {
        let json = std::fs::read_to_string(&schema_path).unwrap_or_default();
        match latent_schema::Schema::from_json(&json) {
            Ok(s) => latent_data::Store::new(s),
            Err(e) => fail(format!("invalid schema {}: {e}", schema_path.display())),
        }
    };

    let mut app = latent_ui::App::new(store);
    app.engine.dirty = upgraded;
    app.base_dir = dir;
    app.on_save = Some(Box::new(move |s| {
        latent_data::fs::save(s, &schema_path, &data_path).map_err(|e| e.to_string())
    }));
    let mut options = eframe::NativeOptions::default();
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        options.viewport = options.viewport.with_icon(icon);
    }
    eframe::run_native("Latent", options, Box::new(|_| Ok(Box::new(app))))
}
