//! Face detection (InsightFace SCRFD) and alignment for ArcFace-style embedding models.
//!
//! Models in `models/`: `face-detect.onnx` + `.json` (`"kind":"face-detect"`, `"size":640`) and
//! `face-embed.onnx` + `.json` (`"kind":"face-embed"`, `"size":112`), loaded like any other [`OnnxModel`].
use crate::{Manifest, VisionError};
use image::{DynamicImage, Rgb, RgbImage};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::Mutex;

const STRIDES: [usize; 3] = [8, 16, 32];
const ANCHORS_PER_CELL: usize = 2;
const NMS_IOU: f32 = 0.4;
/// ArcFace reference landmarks in a 112x112 crop: eyes, nose, mouth corners.
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];
pub const ALIGNED: u32 = 112;

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// `[x1, y1, x2, y2]` in source image pixels.
    pub bbox: [f32; 4],
    /// Left eye, right eye, nose, left and right mouth corner.
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

impl Face {
    pub fn width(&self) -> f32 {
        self.bbox[2] - self.bbox[0]
    }
    pub fn height(&self) -> f32 {
        self.bbox[3] - self.bbox[1]
    }
}

pub struct FaceDetector {
    size: u32,
    session: Mutex<Session>,
}

impl FaceDetector {
    /// Loads `<name>.onnx` with `<name>.json` (`size` = square network input, 640 for SCRFD).
    pub fn load(onnx: &Path) -> Result<Self, VisionError> {
        let m: Manifest = serde_json::from_slice(&std::fs::read(onnx.with_extension("json"))?)?;
        let err = |e: ort::Error| VisionError::Model(e.to_string());
        let session = Session::builder()
            .map_err(err)?
            .commit_from_file(onnx)
            .map_err(err)?;
        Ok(Self {
            size: m.size,
            session: Mutex::new(session),
        })
    }

    /// Faces with `score >= min_score`, best first. The image is scaled to fit the network input (aspect kept, zero padding).
    pub fn detect(&self, img: &DynamicImage, min_score: f32) -> Result<Vec<Face>, VisionError> {
        let err = |e: ort::Error| VisionError::Model(e.to_string());
        let n = self.size as usize;
        let scale = (n as f32 / img.width() as f32).min(n as f32 / img.height() as f32);
        let (w, h) = (
            ((img.width() as f32 * scale).round() as u32).max(1),
            ((img.height() as f32 * scale).round() as u32).max(1),
        );
        let rgb = img
            .resize_exact(w, h, image::imageops::FilterType::Triangle)
            .to_rgb8();
        // Padding is black, i.e. (0 - 127.5) / 128 after normalisation.
        let mut data = vec![-127.5 / 128.0; 3 * n * n];
        for (x, y, p) in rgb.enumerate_pixels() {
            for c in 0..3 {
                data[c * n * n + y as usize * n + x as usize] = (p[c] as f32 - 127.5) / 128.0;
            }
        }
        let input = Tensor::from_array(([1i64, 3, n as i64, n as i64], data)).map_err(err)?;
        let mut session = self.session.lock().unwrap();
        let outputs = session.run(ort::inputs![input]).map_err(err)?;
        // Outputs: scores, boxes, keypoints for each stride (9 in total).
        let mut tensors: Vec<Vec<f32>> = vec![];
        for i in 0..outputs.len() {
            tensors.push(
                outputs[i]
                    .try_extract_tensor::<f32>()
                    .map_err(err)?
                    .1
                    .to_vec(),
            );
        }
        if tensors.len() != 9 {
            return Err(VisionError::Model(format!(
                "face detector has {} outputs, expected 9 (SCRFD with keypoints)",
                tensors.len()
            )));
        }
        let mut faces = vec![];
        for (k, &stride) in STRIDES.iter().enumerate() {
            faces.extend(decode(
                &tensors[k],
                &tensors[k + 3],
                &tensors[k + 6],
                stride,
                n / stride,
                min_score,
            ));
        }
        for f in &mut faces {
            f.bbox.iter_mut().for_each(|v| *v /= scale);
            f.landmarks.iter_mut().flatten().for_each(|v| *v /= scale);
        }
        Ok(nms(faces, NMS_IOU))
    }
}

/// Decodes one SCRFD stride: `scores` `[cells*2]`, `boxes` `[cells*2*4]` (distances in stride units from the anchor), `kps` `[cells*2*10]`.
fn decode(
    scores: &[f32],
    boxes: &[f32],
    kps: &[f32],
    stride: usize,
    grid: usize,
    min_score: f32,
) -> Vec<Face> {
    let s = stride as f32;
    let mut out = vec![];
    for (i, &score) in scores.iter().enumerate() {
        if score < min_score || i >= grid * grid * ANCHORS_PER_CELL {
            continue;
        }
        let cell = i / ANCHORS_PER_CELL;
        let (cx, cy) = ((cell % grid) as f32 * s, (cell / grid) as f32 * s);
        let b = &boxes[i * 4..i * 4 + 4];
        let k = &kps[i * 10..i * 10 + 10];
        let mut landmarks = [[0.0; 2]; 5];
        for (j, l) in landmarks.iter_mut().enumerate() {
            *l = [cx + k[2 * j] * s, cy + k[2 * j + 1] * s];
        }
        out.push(Face {
            bbox: [cx - b[0] * s, cy - b[1] * s, cx + b[2] * s, cy + b[3] * s],
            landmarks,
            score,
        });
    }
    out
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (w, h) = (
        (a[2].min(b[2]) - a[0].max(b[0])).max(0.0),
        (a[3].min(b[3]) - a[1].max(b[1])).max(0.0),
    );
    let inter = w * h;
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Non-maximum suppression: keeps the best face of every group of boxes overlapping by more than `max_iou`.
pub fn nms(mut faces: Vec<Face>, max_iou: f32) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = vec![];
    for f in faces {
        if kept.iter().all(|k| iou(&k.bbox, &f.bbox) <= max_iou) {
            kept.push(f);
        }
    }
    kept
}

/// Similarity transform `dst = [[a, -b], [b, a]] * src + t` fitted to the point pairs (least squares), as `[a, b, tx, ty]`.
fn similarity(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [f32; 4] {
    let mean = |p: &[[f32; 2]; 5]| {
        [
            p.iter().map(|v| v[0]).sum::<f32>() / 5.0,
            p.iter().map(|v| v[1]).sum::<f32>() / 5.0,
        ]
    };
    let (ms, md) = (mean(src), mean(dst));
    let (mut num_a, mut num_b, mut den) = (0.0, 0.0, 0.0);
    for (s, d) in src.iter().zip(dst) {
        let (sx, sy, dx, dy) = (s[0] - ms[0], s[1] - ms[1], d[0] - md[0], d[1] - md[1]);
        num_a += sx * dx + sy * dy;
        num_b += sx * dy - sy * dx;
        den += sx * sx + sy * sy;
    }
    let (a, b) = if den > 0.0 {
        (num_a / den, num_b / den)
    } else {
        (1.0, 0.0)
    };
    [
        a,
        b,
        md[0] - (a * ms[0] - b * ms[1]),
        md[1] - (b * ms[0] + a * ms[1]),
    ]
}

/// The face warped onto the ArcFace template (112x112, bilinear, black outside the image).
pub fn align(img: &DynamicImage, face: &Face) -> DynamicImage {
    let [a, b, tx, ty] = similarity(&face.landmarks, &TEMPLATE);
    let det = a * a + b * b;
    let src = img.to_rgb8();
    let mut out = RgbImage::new(ALIGNED, ALIGNED);
    for (x, y, p) in out.enumerate_pixels_mut() {
        // Inverse mapping: source = R^-1 * (dst - t).
        let (dx, dy) = (x as f32 - tx, y as f32 - ty);
        let (sx, sy) = ((a * dx + b * dy) / det, (-b * dx + a * dy) / det);
        *p = bilinear(&src, sx, sy);
    }
    DynamicImage::ImageRgb8(out)
}

fn bilinear(img: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: i32, yi: i32| {
        if xi < 0 || yi < 0 || xi >= w || yi >= h {
            [0.0; 3]
        } else {
            let p = img.get_pixel(xi as u32, yi as u32);
            [p[0] as f32, p[1] as f32, p[2] as f32]
        }
    };
    let (p00, p10, p01, p11) = (
        at(x0 as i32, y0 as i32),
        at(x0 as i32 + 1, y0 as i32),
        at(x0 as i32, y0 as i32 + 1),
        at(x0 as i32 + 1, y0 as i32 + 1),
    );
    let mut px = [0u8; 3];
    for c in 0..3 {
        let top = p00[c] * (1.0 - fx) + p10[c] * fx;
        let bottom = p01[c] * (1.0 - fx) + p11[c] * fx;
        px[c] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    Rgb(px)
}

/// Scales `v` to unit length (unchanged if it is all zeros).
pub fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(bbox: [f32; 4], score: f32) -> Face {
        Face {
            bbox,
            landmarks: TEMPLATE,
            score,
        }
    }

    #[test]
    fn nms_keeps_best_of_overlapping_boxes() {
        let r = nms(
            vec![
                face([0.0, 0.0, 10.0, 10.0], 0.6),
                face([1.0, 1.0, 11.0, 11.0], 0.9),
                face([50.0, 50.0, 60.0, 60.0], 0.5),
            ],
            0.4,
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].score, 0.9);
        assert_eq!(r[1].score, 0.5);
    }

    #[test]
    fn template_aligns_to_itself() {
        let t = similarity(&TEMPLATE, &TEMPLATE);
        assert!(
            (t[0] - 1.0).abs() < 1e-5
                && t[1].abs() < 1e-5
                && t[2].abs() < 1e-3
                && t[3].abs() < 1e-3
        );
    }

    #[test]
    fn similarity_recovers_scale_rotation_and_shift() {
        // dst = 2 * rot90(src) + (5, -3): a = 0, b = 2.
        let src = TEMPLATE;
        let mut dst = [[0.0; 2]; 5];
        for (d, s) in dst.iter_mut().zip(&src) {
            *d = [-2.0 * s[1] + 5.0, 2.0 * s[0] - 3.0];
        }
        let t = similarity(&src, &dst);
        assert!(
            t[0].abs() < 1e-4
                && (t[1] - 2.0).abs() < 1e-4
                && (t[2] - 5.0).abs() < 1e-3
                && (t[3] + 3.0).abs() < 1e-3,
            "{t:?}"
        );
    }

    #[test]
    fn decode_places_box_around_anchor() {
        // Stride 8, 2x2 grid; anchor 6 is cell 3 (x = 1, y = 1) => centre (8, 8).
        let mut scores = vec![0.0; 8];
        scores[6] = 0.9;
        let mut boxes = vec![0.0; 32];
        boxes[24..28].copy_from_slice(&[1.0, 2.0, 3.0, 4.0]);
        let kps = vec![0.5; 80];
        let f = decode(&scores, &boxes, &kps, 8, 2, 0.5);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].bbox, [0.0, -8.0, 32.0, 40.0]);
        assert_eq!(f[0].landmarks[0], [12.0, 12.0]);
    }

    #[test]
    fn align_gives_template_sized_crop() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(200, 200, Rgb([10, 20, 30])));
        let f = Face {
            bbox: [0.0, 0.0, 200.0, 200.0],
            landmarks: [
                [60.0, 80.0],
                [140.0, 80.0],
                [100.0, 120.0],
                [70.0, 160.0],
                [130.0, 160.0],
            ],
            score: 1.0,
        };
        let c = align(&img, &f);
        assert_eq!((c.width(), c.height()), (ALIGNED, ALIGNED));
        assert_eq!(c.to_rgb8().get_pixel(56, 56).0, [10, 20, 30]);
    }

    #[test]
    fn normalize_gives_unit_length() {
        let v = normalize(vec![3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        assert_eq!(normalize(vec![0.0, 0.0]), vec![0.0, 0.0]);
    }
}
