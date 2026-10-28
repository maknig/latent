#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("Build this crate for the web: `trunk build` / `trunk serve` in apps/web.");
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("app_canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("canvas #app_canvas");
        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| {
                    // Prefer the autosaved archive in localStorage, else the sample schema.
                    let saved = cc
                        .storage
                        .and_then(|s| Some((s.get_string("schema")?, s.get_string("data")?)));
                    let store = saved
                        .and_then(|(s, d)| latent_data::Store::load(&s, &d).ok().map(|x| x.0))
                        .unwrap_or_else(|| {
                            let s = include_str!("../../../schemas/latent.json");
                            latent_data::Store::new(
                                latent_schema::Schema::from_json(s).expect("schema"),
                            )
                        });
                    let mut app = latent_ui::App::new(store);
                    app.persist = true;
                    Ok(Box::new(app))
                }),
            )
            .await
            .expect("failed to start eframe");
    });
}
