use fa_vision::{score_tags, Model, OnnxModel, TextModel};
use std::path::PathBuf;

// usage: clip <models dir> <image> <tag>...
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&a[0]);
    let img = OnnxModel::load(&dir.join("clip-image.onnx")).unwrap();
    let txt = TextModel::load(&dir.join("clip-text.onnx")).unwrap();
    let q = img.run(&image::open(&a[1]).unwrap()).unwrap();
    let texts: Vec<(String, Vec<f32>)> = a[2..].iter().map(|t| (t.clone(), txt.embed(&format!("a photo of {t}")).unwrap())).collect();
    println!("dims {} {}", q.len(), texts[0].1.len());
    for (t, s) in score_tags(&q, &texts, 100.0) {
        println!("{s:.3} {t}");
    }
}
