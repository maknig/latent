use fa_vision::{align, cosine, normalize, FaceDetector, Model, OnnxModel};
use std::path::PathBuf;

// usage: faces <models dir> <image> [<image2>]
// Prints the detected faces; with a second image also the cosine between the best faces of both.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&a[0]);
    let detector = FaceDetector::load(&dir.join("face-detect.onnx")).unwrap();
    let embedder = OnnxModel::load(&dir.join("face-embed.onnx")).unwrap();
    let mut best = vec![];
    // FACE_CROPS=<dir> saves every aligned crop for a visual check.
    let crops = std::env::var("FACE_CROPS").ok();
    for (n, path) in a[1..].iter().enumerate() {
        let img = image::open(path).unwrap();
        let faces = detector.detect(&img, 0.5).unwrap();
        println!(
            "{path}: {}x{}, {} face(s)",
            img.width(),
            img.height(),
            faces.len()
        );
        for (i, f) in faces.iter().enumerate() {
            if let Some(d) = &crops {
                align(&img, f)
                    .save(format!("{d}/face-{n}-{i}.png"))
                    .unwrap();
            }
            let v = normalize(embedder.run(&align(&img, f)).unwrap());
            println!(
                "  score {:.2} box {:?} size {:.0}x{:.0} dim {}",
                f.score,
                f.bbox.map(|x| x.round()),
                f.width(),
                f.height(),
                v.len()
            );
        }
        if let Some(f) = faces.first() {
            best.push(normalize(embedder.run(&align(&img, f)).unwrap()));
        }
    }
    if best.len() == 2 {
        println!("cosine of best faces: {:.3}", cosine(&best[0], &best[1]));
    }
}
