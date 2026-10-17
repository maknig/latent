use std::path::PathBuf;

fn main() -> eframe::Result {
    let base = std::env::args().nth(1).unwrap_or_else(|| "filmarchiv".into());
    let schema_path = PathBuf::from(format!("{base}.schema.json"));
    let data_path = PathBuf::from(format!("{base}.data.json"));

    let store = if schema_path.exists() && data_path.exists() {
        match fa_data::fs::load(&schema_path, &data_path) {
            Ok((s, w)) => {
                for m in w { eprintln!("warning: {m}"); }
                s
            }
            Err(e) => { eprintln!("cannot load: {e}"); std::process::exit(1) }
        }
    } else {
        let schema_json = if schema_path.exists() {
            std::fs::read_to_string(&schema_path).expect("read schema")
        } else {
            include_str!("../../../schemas/filmarchiv.json").to_string()
        };
        fa_data::Store::new(fa_schema::Schema::from_json(&schema_json).expect("schema"))
    };

    let mut app = fa_ui::App::new(store);
    app.base_dir = data_path.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf()).unwrap_or_else(|| ".".into());
    app.on_save = Some(Box::new(move |s| {
        fa_data::fs::save(s, &schema_path, &data_path).map_err(|e| e.to_string())
    }));
    eframe::run_native("FilmArchiv", eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(app))))
}
