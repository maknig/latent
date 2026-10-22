//! Pluggable vision models: image -> feature vector, plus plain-Rust similarity and clustering.

mod cache;
mod cluster;
mod onnx;
mod tags;

pub use cache::EmbeddingCache;
pub use cluster::{cluster, cosine, most_similar};
pub use onnx::{Manifest, OnnxModel};
pub use tags::{split_tags, suggest_from_neighbours};

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
