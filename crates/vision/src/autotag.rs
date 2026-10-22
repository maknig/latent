//! Tag suggestions for one image, merged from two sources:
//! - tags of its most similar tagged images (embedding model, e.g. DINOv2), and
//! - CLIP zero-shot scores of a vocabulary (`clip-image` + `clip-text` in `models/`).
//!
//! Embeddings are cached in `filmarchiv.vision.json` next to the data.
use crate::{score_tags, suggest_from_neighbours, EmbeddingCache, Model, OnnxModel, TextModel};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Neighbours below this cosine similarity do not vote.
const MIN_SIMILARITY: f32 = 0.5;
const NEIGHBOURS: usize = 10;
/// CLIP's logit scale.
const CLIP_SCALE: f32 = 100.0;

/// Common photo tags (German tag, English CLIP prompt), offered next to the archive's own tags.
pub const DEFAULT_TAGS: &[(&str, &str)] = &[
    ("Berge", "mountains"), ("Schnee", "snow"), ("See", "a lake"), ("Meer", "the sea"), ("Strand", "a beach"),
    ("Wald", "a forest"), ("Wiese", "a meadow"), ("Fluss", "a river"), ("Wasserfall", "a waterfall"), ("Himmel", "the sky"),
    ("Wolken", "clouds"), ("Sonnenuntergang", "a sunset"), ("Nebel", "fog"), ("Landschaft", "a landscape"), ("Wüste", "a desert"),
    ("Blumen", "flowers"), ("Baum", "a tree"), ("Herbst", "autumn leaves"), ("Winter", "a winter scene"), ("Garten", "a garden"),
    ("Vogel", "a bird"), ("Möwe", "a seagull"), ("Katze", "a cat"), ("Hund", "a dog"), ("Pferd", "a horse"),
    ("Kuh", "a cow"), ("Schaf", "sheep"), ("Insekt", "an insect"), ("Schmetterling", "a butterfly"), ("Fisch", "a fish"),
    ("Porträt", "a portrait of a person"), ("Kind", "a child"), ("Menschen", "people"), ("Gruppe", "a group of people"), ("Paar", "a couple"),
    ("Hände", "hands"), ("Gesicht", "a face"), ("Tanz", "dancers"), ("Musik", "a musician"), ("Sport", "sports"),
    ("Stadt", "a city"), ("Strasse", "a street"), ("Gebäude", "a building"), ("Architektur", "architecture"), ("Kirche", "a church"),
    ("Brücke", "a bridge"), ("Turm", "a tower"), ("Fenster", "a window"), ("Tür", "a door"), ("Treppe", "stairs"),
    ("Dorf", "a village"), ("Markt", "a market"), ("Hafen", "a harbour"), ("Boot", "a boat"), ("Schiff", "a ship"),
    ("Auto", "a car"), ("Zug", "a train"), ("Fahrrad", "a bicycle"), ("Flugzeug", "an airplane"), ("Motorrad", "a motorcycle"),
    ("Nacht", "a night scene"), ("Schatten", "shadows"), ("Gegenlicht", "backlit scene"), ("Silhouette", "a silhouette"), ("Spiegelung", "a reflection"),
    ("Makro", "a macro photo"), ("Detail", "a close-up detail"), ("Muster", "a pattern"), ("Struktur", "a texture"), ("Abstrakt", "an abstract photo"),
    ("Essen", "food"), ("Tisch", "a table"), ("Innenraum", "an interior room"), ("Stillleben", "a still life"), ("Kunst", "an artwork"),
    ("Schrift", "text on a sign"), ("Industrie", "an industrial site"), ("Ruine", "a ruin"), ("Friedhof", "a cemetery"), ("Reise", "travel"),
    ("Schwarzweiss", "a black and white photo"), ("Testbild", "a test chart"), ("Labor", "a laboratory"), ("Computer", "a computer"),
];

fn embedding(model: &OnnxModel, cache: &mut EmbeddingCache, base: &Path, rel: &str) -> Result<Vec<f32>, String> {
    let full = base.join(rel);
    let mtime = std::fs::metadata(&full).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    if let Some(v) = cache.get(model.id(), rel, mtime) {
        return Ok(v.to_vec());
    }
    let v = image::open(&full).map_err(|e| format!("{rel}: {e}")).and_then(|im| model.run(&im).map_err(|e| e.to_string()))?;
    cache.insert(model.id(), rel, mtime, v.clone());
    Ok(v)
}

/// The first model file in `dir` of the given `kind` (`None` = plain embedding model).
pub fn find_kind(dir: &Path, kind: Option<&str>) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "onnx") && p.with_extension("json").exists() && crate::kind_of(p).as_deref() == kind)
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Optional `tag_prompts.json`: tag -> English prompt for CLIP; overrides the built-in prompts.
fn aliases(base: &Path) -> HashMap<String, String> {
    std::fs::read(base.join("tag_prompts.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn neighbours(models: &Path, base: &Path, cache: &mut EmbeddingCache, query: &str, tagged: &[(String, Vec<String>)]) -> Result<Vec<(String, f32)>, String> {
    let Some(onnx) = find_kind(models, None) else { return Ok(vec![]) };
    let model = OnnxModel::load(&onnx).map_err(|e| e.to_string())?;
    let q = embedding(&model, cache, base, query)?;
    let mut vectors = vec![];
    for (rel, tags) in tagged.iter().filter(|(rel, _)| rel != query) {
        // Unreadable images are skipped, not fatal.
        if let Ok(v) = embedding(&model, cache, base, rel) {
            vectors.push((v, tags.clone()));
        }
    }
    Ok(suggest_from_neighbours(&q, &vectors, NEIGHBOURS, MIN_SIMILARITY))
}

fn clip(models: &Path, base: &Path, cache: &mut EmbeddingCache, query: &str, vocabulary: &[(String, String)]) -> Result<Vec<(String, f32)>, String> {
    let (Some(img), Some(txt)) = (find_kind(models, Some("clip-image")), find_kind(models, Some("clip-text"))) else { return Ok(vec![]) };
    let image_model = OnnxModel::load(&img).map_err(|e| e.to_string())?;
    let text_model = TextModel::load(&txt).map_err(|e| e.to_string())?;
    let q = embedding(&image_model, cache, base, query)?;
    let mut texts = vec![];
    for (tag, english) in vocabulary {
        let prompt = format!("a photo of {english}");
        // Text embeddings are cached by prompt (mtime is unused here).
        let v = match cache.get(text_model.id(), &prompt, 0) {
            Some(v) => v.to_vec(),
            None => {
                let v = text_model.embed(&prompt).map_err(|e| e.to_string())?;
                cache.insert(text_model.id(), &prompt, 0, v.clone());
                v
            }
        };
        texts.push((tag.clone(), v));
    }
    Ok(score_tags(&q, &texts, CLIP_SCALE))
}

/// Merges both sources by tag (case-insensitive): the better score wins, tags found by both get a bonus.
fn merge(a: Vec<(String, f32)>, b: Vec<(String, f32)>) -> Vec<(String, f32)> {
    let mut out = a;
    for (tag, s) in b {
        match out.iter_mut().find(|(t, _)| t.to_lowercase() == tag.to_lowercase()) {
            Some(e) => e.1 = (e.1.max(s) + 0.1).min(1.0),
            None => out.push((tag, s)),
        }
    }
    out.sort_by(|x, y| y.1.total_cmp(&x.1));
    out
}

/// Suggestions (tag, score) for image `query` (archive-relative). `tagged` are the images that already have tags.
pub fn suggest(base: &Path, query: &str, tagged: &[(String, Vec<String>)]) -> Result<Vec<(String, f32)>, String> {
    let models = base.join("models");
    if find_kind(&models, None).is_none() && find_kind(&models, Some("clip-image")).is_none() {
        return Err(format!("No model in {}", models.display()));
    }
    let cache_path = base.join("filmarchiv.vision.json");
    let mut cache = EmbeddingCache::load(&cache_path);
    let prompts = aliases(base);
    // Vocabulary: the archive's tags first, then the built-in ones; first spelling wins.
    let mut vocabulary: Vec<(String, String)> = vec![];
    let known = |v: &[(String, String)], tag: &str| v.iter().any(|(t, _)| t.to_lowercase() == tag.to_lowercase());
    let own = tagged.iter().flat_map(|(_, t)| t.iter().map(|t| (t.clone(), None)));
    let builtin = DEFAULT_TAGS.iter().map(|(t, e)| (t.to_string(), Some(e.to_string())));
    for (tag, builtin_prompt) in own.chain(builtin) {
        if !known(&vocabulary, &tag) {
            let english = prompts.get(&tag).cloned().or(builtin_prompt).unwrap_or_else(|| tag.clone());
            vocabulary.push((tag, english));
        }
    }
    let near = neighbours(&models, base, &mut cache, query, tagged)?;
    let by_clip = clip(&models, base, &mut cache, query, &vocabulary)?;
    cache.save(&cache_path).map_err(|e| e.to_string())?;
    Ok(merge(near, by_clip))
}
