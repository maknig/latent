// usage: autotag <archive> <image rel path>...   (tags are read from image_info in filmarchiv.data.json)
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let base = std::path::PathBuf::from(&a[0]);
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(base.join("filmarchiv.data.json")).unwrap()).unwrap();
    let rows = data["tables"]["image_info"]["rows"].as_array().or_else(|| data["image_info"]["rows"].as_array()).cloned().unwrap_or_default();
    let tagged: Vec<(String, Vec<String>)> = rows
        .iter()
        .map(|r| (r["source_image"].as_str().unwrap_or("").to_string(), fa_vision::split_tags(r["tags"].as_str().unwrap_or(""))))
        .filter(|(p, t)| !p.is_empty() && !t.is_empty())
        .collect();
    println!("{} tagged images", tagged.len());
    for q in &a[1..] {
        let t = std::time::Instant::now();
        match fa_vision::suggest_tags(&base, q, &tagged) {
            Ok(r) => println!("{q} ({:?})\n{}", t.elapsed(), r.iter().take(8).map(|(t, s)| format!("   {s:.2} {t}")).collect::<Vec<_>>().join("\n")),
            Err(e) => println!("{q}: {e}"),
        }
    }
}
