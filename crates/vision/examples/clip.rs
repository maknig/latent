use fa_vision::{cosine, Model, OnnxModel, TextModel, DEFAULT_TAGS};
use std::path::PathBuf;

// usage: clip <models dir> <image> [<tag>...]
// Without tags the built-in vocabulary (DEFAULT_TAGS) is scored with its English prompts.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&a[0]);
    let img = OnnxModel::load(&dir.join("clip-image.onnx")).unwrap();
    let txt = TextModel::load(&dir.join("clip-text.onnx")).unwrap();
    let mut im = image::open(&a[1]).unwrap();
    if std::env::var("INVERT").is_ok() {
        im.invert();
    }
    let q = img.run(&im).unwrap();
    // (label, English phrase)
    let vocab: Vec<(String, String)> = if a.len() > 2 {
        a[2..].iter().map(|t| (t.clone(), t.clone())).collect()
    } else {
        DEFAULT_TAGS.iter().map(|(t, e)| (t.to_string(), e.to_string())).collect()
    };
    // (label, prompt, cosine)
    let mut rows: Vec<(String, String, f32)> = vocab
        .into_iter()
        .map(|(t, e)| {
            let prompt = format!("a photo of {e}");
            let c = cosine(&q, &txt.embed(&prompt).unwrap());
            (t, prompt, c)
        })
        .collect();
    let logits: Vec<f32> = rows.iter().map(|r| 100.0 * r.2).collect();
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let sum: f32 = logits.iter().map(|l| (l - max).exp()).sum();
    println!("image dims {}", q.len());
    println!("{:>6} {:>7} {:>6}  {:<16} prompt", "prob", "logit", "cos", "tag");
    rows.sort_by(|x, y| y.2.total_cmp(&x.2));
    for (t, prompt, c) in rows {
        let l = 100.0 * c;
        println!("{:>6.3} {:>7.2} {:>6.3}  {:<16} {}", (l - max).exp() / sum, l, c, t, prompt);
    }
}
