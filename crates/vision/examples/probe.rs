use fa_vision::{most_similar, Model, OnnxModel};
use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let model = OnnxModel::load(&PathBuf::from(&a[0])).unwrap();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&a[1]).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    files.retain(|f| image::open(f).is_ok());
    let t = std::time::Instant::now();
    let vecs: Vec<Vec<f32>> = files.iter().map(|f| model.run(&image::open(f).unwrap()).unwrap()).collect();
    println!("dim {} , {} images in {:?}", vecs[0].len(), vecs.len(), t.elapsed());
    for q in [0, 8, vecs.len() - 1] {
        println!("query {}", files[q].file_name().unwrap().to_string_lossy());
        for (i, s) in most_similar(&vecs[q], &vecs, 4) {
            println!("   {s:.3} {}", files[i].file_name().unwrap().to_string_lossy());
        }
    }
}
