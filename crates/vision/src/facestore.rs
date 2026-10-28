//! Faces found in the archive's images, with the people they were assigned to.
//!
//! Stored in `latent.faces.json` next to the data (separate from the embedding cache: face vectors are biometric data
//! and the file also holds the user's naming work).
use crate::{align, normalize, FaceDetector, Model, OnnxModel, VisionError};
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Detection score below which a face is ignored.
pub const MIN_SCORE: f32 = 0.5;
/// Faces smaller than this (shorter side, source pixels) are too blurry to recognise and are ignored.
pub const MIN_SIDE: f32 = 40.0;
/// Cosine similarity from which two faces count as the same person (same person: about 0.5, different people: below 0.3).
pub const FACE_MATCH: f32 = 0.4;
/// A person's score is the mean of their best this-many cosines.
const TOP_K: usize = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredFace {
    /// `[x1, y1, x2, y2]` in source image pixels.
    pub bbox: [f32; 4],
    /// L2-normalised face embedding.
    pub embedding: Vec<f32>,
    pub person: Option<u32>,
}

#[derive(Default, Serialize, Deserialize)]
struct ImageFaces {
    mtime: u64,
    faces: Vec<StoredFace>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct FaceStore {
    #[serde(default)]
    images: BTreeMap<String, ImageFaces>,
    #[serde(default)]
    people: BTreeMap<u32, String>,
}

impl FaceStore {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomic save (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), VisionError> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The faces of `image` if they were found for this file version (`mtime`).
    pub fn faces(&self, image: &str, mtime: u64) -> Option<&[StoredFace]> {
        let e = self.images.get(image)?;
        (e.mtime == mtime).then_some(e.faces.as_slice())
    }

    /// Stores the faces of `image`. Assignments survive a re-detection of the same number of faces.
    pub fn set_faces(&mut self, image: &str, mtime: u64, mut faces: Vec<StoredFace>) {
        if let Some(old) = self.images.get(image) {
            if old.faces.len() == faces.len() {
                for (f, o) in faces.iter_mut().zip(&old.faces) {
                    f.person = o.person;
                }
            }
        }
        self.images
            .insert(image.to_string(), ImageFaces { mtime, faces });
    }

    /// Every stored face as (image, index in image, face), in a stable order.
    pub fn all(&self) -> impl Iterator<Item = (&str, usize, &StoredFace)> {
        self.images.iter().flat_map(|(p, e)| {
            e.faces
                .iter()
                .enumerate()
                .map(move |(i, f)| (p.as_str(), i, f))
        })
    }

    pub fn people(&self) -> impl Iterator<Item = (u32, &str)> {
        self.people.iter().map(|(id, n)| (*id, n.as_str()))
    }

    pub fn person_name(&self, id: u32) -> Option<&str> {
        self.people.get(&id).map(String::as_str)
    }

    /// Id of the person called `name` (case-insensitive); a new person is created if there is none.
    pub fn person_id(&mut self, name: &str) -> u32 {
        let lower = name.trim().to_lowercase();
        if let Some((id, _)) = self.people.iter().find(|(_, n)| n.to_lowercase() == lower) {
            return *id;
        }
        let id = self.people.keys().next_back().map_or(1, |m| m + 1);
        self.people.insert(id, name.trim().to_string());
        id
    }

    /// Assigns face `index` of `image` to `person` (`None` removes the assignment).
    pub fn assign(&mut self, image: &str, index: usize, person: Option<u32>) {
        if let Some(f) = self
            .images
            .get_mut(image)
            .and_then(|e| e.faces.get_mut(index))
        {
            f.person = person;
        }
    }

    /// Drops people without any face and images that are no longer in `keep`.
    pub fn prune(&mut self, keep: &dyn Fn(&str) -> bool) {
        self.images.retain(|p, _| keep(p));
        let used: Vec<u32> = self.all().filter_map(|(_, _, f)| f.person).collect();
        self.people.retain(|id, _| used.contains(id));
    }

    /// Named people ranked by similarity to `query`, skipping the faces of `skip_image` (the query image itself).
    pub fn match_people(
        &self,
        query: &[f32],
        skip_image: &str,
        min_cosine: f32,
    ) -> Vec<(u32, f32)> {
        let gallery: Vec<(u32, &[f32])> = self
            .images
            .iter()
            .filter(|(p, _)| p.as_str() != skip_image)
            .flat_map(|(_, e)| e.faces.iter())
            .filter_map(|f| f.person.map(|p| (p, f.embedding.as_slice())))
            .collect();
        match_people(query, &gallery, min_cosine)
    }
}

/// Per person the mean of their `TOP_K` best cosines to `query`; people below `min_cosine` are dropped. Best first.
pub fn match_people(query: &[f32], gallery: &[(u32, &[f32])], min_cosine: f32) -> Vec<(u32, f32)> {
    let mut per_person: BTreeMap<u32, Vec<f32>> = BTreeMap::new();
    for (person, v) in gallery {
        per_person
            .entry(*person)
            .or_default()
            .push(crate::cosine(query, v));
    }
    let mut out: Vec<(u32, f32)> = per_person
        .into_iter()
        .map(|(p, mut c)| {
            c.sort_by(|a, b| b.total_cmp(a));
            c.truncate(TOP_K);
            (p, c.iter().sum::<f32>() / c.len() as f32)
        })
        .filter(|(_, s)| *s >= min_cosine)
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// Detects, aligns and embeds the faces of one image, skipping small and unsure ones.
/// Each face comes with its aligned 112x112 crop (for thumbnails).
pub fn embed_faces(
    detector: &FaceDetector,
    embedder: &OnnxModel,
    img: &DynamicImage,
) -> Result<Vec<(StoredFace, DynamicImage)>, VisionError> {
    let mut out = vec![];
    for f in detector.detect(img, MIN_SCORE)? {
        if f.width().min(f.height()) < MIN_SIDE {
            continue;
        }
        let crop = align(img, &f);
        let embedding = normalize(embedder.run(&crop)?);
        out.push((
            StoredFace {
                bbox: f.bbox,
                embedding,
                person: None,
            },
            crop,
        ));
    }
    Ok(out)
}

/// Where the thumbnail of face `index` of `image` (archive-relative) is kept: a cache below `.thumbs`, written when the face is found.
pub fn face_crop_path(base: &Path, image: &str, index: usize) -> std::path::PathBuf {
    base.join(".thumbs")
        .join("faces")
        .join(format!("{}_{index}.png", image.replace(['/', '\\'], "_")))
}

/// Writes the aligned crops of the faces of `image` to `face_crop_path` and returns the faces without them.
pub fn save_crops(
    base: &Path,
    image: &str,
    found: Vec<(StoredFace, DynamicImage)>,
) -> Vec<StoredFace> {
    found
        .into_iter()
        .enumerate()
        .map(|(i, (face, crop))| {
            let p = face_crop_path(base, image, i);
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // The crop is only a thumbnail: a failed write must not fail the scan.
            let _ = crop.save(&p);
            face
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(v: [f32; 2], person: Option<u32>) -> StoredFace {
        StoredFace {
            bbox: [0.0; 4],
            embedding: v.to_vec(),
            person,
        }
    }

    #[test]
    fn person_ids_are_case_insensitive_and_stable() {
        let mut s = FaceStore::default();
        let a = s.person_id("Lara");
        assert_eq!(s.person_id(" lara "), a);
        assert_ne!(s.person_id("Jon"), a);
        assert_eq!(s.person_name(a), Some("Lara"));
    }

    #[test]
    fn faces_are_invalidated_by_mtime_and_keep_assignments() {
        let mut s = FaceStore::default();
        let p = s.person_id("Lara");
        s.set_faces(
            "a.png",
            5,
            vec![face([1.0, 0.0], Some(p)), face([0.0, 1.0], None)],
        );
        assert!(s.faces("a.png", 6).is_none());
        // Re-detection with the same face count keeps who is who.
        s.set_faces(
            "a.png",
            6,
            vec![face([1.0, 0.1], None), face([0.1, 1.0], None)],
        );
        assert_eq!(s.faces("a.png", 6).unwrap()[0].person, Some(p));
        // A different count resets it.
        s.set_faces("a.png", 7, vec![face([1.0, 0.0], None)]);
        assert_eq!(s.faces("a.png", 7).unwrap()[0].person, None);
    }

    #[test]
    fn matches_nearest_person_above_threshold_only() {
        let (lara, jon) = (1, 2);
        let g: Vec<(u32, &[f32])> =
            vec![(lara, &[1.0, 0.0]), (lara, &[0.9, 0.1]), (jon, &[0.0, 1.0])];
        let r = match_people(&[1.0, 0.0], &g, 0.4);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, lara);
        assert!(match_people(&[0.7, 0.7], &g, 0.9).is_empty());
    }

    #[test]
    fn query_image_does_not_match_itself() {
        let mut s = FaceStore::default();
        let p = s.person_id("Lara");
        s.set_faces("a.png", 1, vec![face([1.0, 0.0], Some(p))]);
        assert!(s.match_people(&[1.0, 0.0], "a.png", 0.4).is_empty());
        s.set_faces("b.png", 1, vec![face([1.0, 0.0], Some(p))]);
        assert_eq!(s.match_people(&[1.0, 0.0], "a.png", 0.4)[0].0, p);
    }

    #[test]
    fn prune_removes_unused_people_and_missing_images() {
        let mut s = FaceStore::default();
        let (a, b) = (s.person_id("Lara"), s.person_id("Jon"));
        s.set_faces("keep.png", 1, vec![face([1.0, 0.0], Some(a))]);
        s.set_faces("gone.png", 1, vec![face([0.0, 1.0], Some(b))]);
        s.prune(&|p| p == "keep.png");
        assert_eq!(s.people().count(), 1);
        assert!(s.faces("gone.png", 1).is_none());
    }

    #[test]
    fn roundtrip() {
        let mut s = FaceStore::default();
        let p = s.person_id("Lara");
        s.set_faces("a.png", 3, vec![face([1.0, 0.0], Some(p))]);
        let path = std::env::temp_dir().join(format!("latent-faces-{}.json", std::process::id()));
        s.save(&path).unwrap();
        let l = FaceStore::load(&path);
        assert_eq!(l.faces("a.png", 3).unwrap()[0].person, Some(p));
        assert_eq!(l.person_name(p), Some("Lara"));
        let _ = std::fs::remove_file(path);
    }
}
