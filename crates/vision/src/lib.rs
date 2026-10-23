//! Pluggable vision models: image -> feature vector, plus plain-Rust similarity and clustering.

mod autotag;
mod cache;
mod cluster;
mod facestore;
mod faces;
mod onnx;
mod tags;

pub use autotag::{find_kind, suggest as suggest_tags, DEFAULT_TAGS};
pub use cache::EmbeddingCache;
pub use cluster::{cluster, cosine, most_similar};
pub use facestore::{embed_faces, face_crop_path, match_people, FaceStore, StoredFace, FACE_MATCH};
pub use faces::{align, nms, normalize, Face, FaceDetector};
pub use onnx::{kind_of, Manifest, OnnxModel, TextModel};
pub use tags::{score_tags, split_tags, suggest_from_neighbours};

use image::DynamicImage;

#[derive(Debug, thiserror::Error)]
pub enum VisionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("model: {0}")]
    Model(String),
    #[error("clustering: {0}")]
    Cluster(String),
}

/// A vision model that turns an image into a feature vector (embedding).
pub trait Model: Send + Sync {
    /// Stable identifier, part of the cache key.
    fn id(&self) -> &str;
    fn run(&self, img: &DynamicImage) -> Result<Vec<f32>, VisionError>;
}
