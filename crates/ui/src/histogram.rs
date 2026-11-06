//! Histogram and black/white points of an image (the histogram is drawn by `latent_image_edit_ui`).
use latent_image_edit_ui::histogram::{marker_points, Histogram, Markers};

/// Histogram plus the darkest, brightest and most neutral spots.
pub struct ImageStats {
    pub hist: Histogram,
    pub markers: Markers,
}

pub fn image_stats(im: &image::RgbaImage) -> ImageStats {
    ImageStats {
        hist: Histogram::of_rgba(im),
        markers: marker_points(im),
    }
}
