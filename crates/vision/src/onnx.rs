use crate::{Model, VisionError};
use image::{imageops::FilterType, DynamicImage};
use ort::session::Session;
use ort::value::Tensor;
use serde::Deserialize;
use std::path::Path;
use std::sync::Mutex;

/// JSON next to the `.onnx` file (`<name>.json`) describing how to feed the model.
#[derive(Deserialize, Clone, Debug)]
pub struct Manifest {
    /// Square input edge in pixels.
    pub size: u32,
    #[serde(default = "half")]
    pub mean: [f32; 3],
    #[serde(default = "half")]
    pub std: [f32; 3],
    /// Output tensor name; first output if absent.
    #[serde(default)]
    pub output: Option<String>,
    /// For token outputs `[1, tokens, dim]`: `"cls"` (first token) or `"mean"`; otherwise the output is used as is.
    #[serde(default)]
    pub pool: Option<String>,
}

fn half() -> [f32; 3] {
    [0.5; 3]
}

pub struct OnnxModel {
    id: String,
    manifest: Manifest,
    session: Mutex<Session>,
}

impl OnnxModel {
    /// Loads `<dir>/<name>.onnx` with `<dir>/<name>.json`.
    pub fn load(onnx: &Path) -> Result<Self, VisionError> {
        let manifest: Manifest =
            serde_json::from_slice(&std::fs::read(onnx.with_extension("json"))?)?;
        let err = |e: ort::Error| VisionError::Model(e.to_string());
        let session = Session::builder()
            .map_err(err)?
            .commit_from_file(onnx)
            .map_err(err)?;
        let id = onnx
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            id,
            manifest,
            session: Mutex::new(session),
        })
    }
}

impl Model for OnnxModel {
    fn id(&self) -> &str {
        &self.id
    }

    fn run(&self, img: &DynamicImage) -> Result<Vec<f32>, VisionError> {
        let m = &self.manifest;
        let err = |e: ort::Error| VisionError::Model(e.to_string());
        let rgb = img
            .resize_exact(m.size, m.size, FilterType::Triangle)
            .to_rgb8();
        let n = (m.size * m.size) as usize;
        let mut data = vec![0f32; 3 * n];
        for (i, p) in rgb.pixels().enumerate() {
            for c in 0..3 {
                data[c * n + i] = (p[c] as f32 / 255.0 - m.mean[c]) / m.std[c];
            }
        }
        let shape = [1i64, 3, m.size as i64, m.size as i64];
        let input = Tensor::from_array((shape, data)).map_err(err)?;
        let mut session = self.session.lock().unwrap();
        let outputs = session.run(ort::inputs![input]).map_err(err)?;
        let out = match &m.output {
            Some(name) => &outputs[name.as_str()],
            None => &outputs[0],
        };
        let (shape, values) = out.try_extract_tensor::<f32>().map_err(err)?;
        match (m.pool.as_deref(), shape.len()) {
            (Some(mode), 3) => {
                let dim = shape[2] as usize;
                Ok(match mode {
                    "cls" => values[..dim].to_vec(),
                    "mean" => {
                        let tokens = values.len() / dim;
                        (0..dim).map(|d| (0..tokens).map(|t| values[t * dim + d]).sum::<f32>() / tokens as f32).collect()
                    }
                    other => return Err(VisionError::Model(format!("unknown pool mode {other}"))),
                })
            }
            (Some(_), n) => Err(VisionError::Model(format!("pool needs a 3-d output, got {n}-d"))),
            (None, _) => Ok(values.to_vec()),
        }
    }
}

/// `"kind"` entry of the JSON beside `onnx` (`"clip-image"`, `"clip-text"`); `None` for a plain embedding model.
pub fn kind_of(onnx: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(onnx.with_extension("json")).ok()?).ok()?;
    v.get("kind")?.as_str().map(String::from)
}

#[derive(Deserialize)]
struct TextManifest {
    /// Tokenizer file next to the model, default `tokenizer.json`.
    #[serde(default = "tokenizer_name")]
    tokenizer: String,
}

fn tokenizer_name() -> String {
    "tokenizer.json".into()
}

/// A text encoder (CLIP): text in, embedding out.
pub struct TextModel {
    id: String,
    tokenizer: tokenizers::Tokenizer,
    session: Mutex<Session>,
}

impl TextModel {
    /// Loads `<name>.onnx` with `<name>.json` and the tokenizer it names.
    pub fn load(onnx: &Path) -> Result<Self, VisionError> {
        let m: TextManifest = serde_json::from_slice(&std::fs::read(onnx.with_extension("json"))?)?;
        let tokenizer_path = onnx.with_file_name(&m.tokenizer);
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path).map_err(|e| VisionError::Model(format!("{}: {e}", tokenizer_path.display())))?;
        let err = |e: ort::Error| VisionError::Model(e.to_string());
        let session = Session::builder().map_err(err)?.commit_from_file(onnx).map_err(err)?;
        let id = onnx.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Self { id, tokenizer, session: Mutex::new(session) })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn embed(&self, text: &str) -> Result<Vec<f32>, VisionError> {
        let err = |e: ort::Error| VisionError::Model(e.to_string());
        let enc = self.tokenizer.encode(text, true).map_err(|e| VisionError::Model(e.to_string()))?;
        let ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        let input = Tensor::from_array(([1i64, ids.len() as i64], ids)).map_err(err)?;
        let mut session = self.session.lock().unwrap();
        let outputs = session.run(ort::inputs![input]).map_err(err)?;
        let (_, values) = outputs[0].try_extract_tensor::<f32>().map_err(err)?;
        Ok(values.to_vec())
    }
}
