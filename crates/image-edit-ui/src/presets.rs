//! Named slider presets (`<config dir>/presets.toml`): the image sliders and the sharpening.
//!
//! A built-in preset ships with the app; a user preset of the same name overrides it. "Film
//! negative", crop and rotation are not part of a preset.

use latent_image_edit::clarity::Clarity;
use latent_image_edit::sharpen::Sharpen;
use latent_image_edit::tone::Adjust;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Name of the preset measured against SilverFast scans of Ilford FP4 (film 71).
pub const SILVERFAST_FP4: &str = "SilverFast look (FP4)";
/// The blank entry: every slider and the sharpening neutral.
pub const DEFAULT: &str = "Default";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub exposure_ev: f32,
    pub black: f32,
    pub white: f32,
    pub midtones: f32,
    pub contrast: f32,
    pub darks: f32,
    pub whites: f32,
    pub warmth: f32,
    pub tint: f32,
    pub saturation: f32,
    pub clarity: f32,
    pub sharpen_amount: f32,
    pub sharpen_radius: f32,
}

impl Default for Preset {
    fn default() -> Self {
        Self::from_values(String::new(), &Adjust::default(), &Sharpen::default())
    }
}

impl Preset {
    pub fn from_values(name: String, a: &Adjust, s: &Sharpen) -> Self {
        Self {
            name,
            exposure_ev: a.exposure_ev,
            black: a.black,
            white: a.white,
            midtones: a.gamma,
            contrast: a.contrast,
            darks: a.darks,
            whites: a.whites,
            warmth: a.warmth,
            tint: a.tint,
            saturation: a.saturation,
            clarity: 0.0,
            sharpen_amount: s.amount,
            sharpen_radius: s.radius,
        }
    }

    /// The image sliders, limited to their ranges (the black point stays below the white point).
    pub fn adjust(&self) -> Adjust {
        let white = self.white.clamp(0.6, 1.0);
        Adjust {
            exposure_ev: self.exposure_ev.clamp(-2.0, 2.0),
            black: self.black.clamp(0.0, 0.4).min(white - 0.1),
            white,
            gamma: self.midtones.clamp(0.5, 2.0),
            warmth: self.warmth.clamp(-1.0, 1.0),
            tint: self.tint.clamp(-1.0, 1.0),
            contrast: self.contrast.clamp(-1.0, 1.0),
            darks: self.darks.clamp(-1.0, 1.0),
            whites: self.whites.clamp(-1.0, 1.0),
            saturation: self.saturation.clamp(-1.0, 1.0),
        }
    }

    /// The same preset with this clarity.
    pub fn with_clarity(mut self, c: &Clarity) -> Self {
        self.clarity = c.amount;
        self
    }

    pub fn clarity(&self) -> Clarity {
        Clarity {
            amount: self.clarity.clamp(-1.0, 1.0),
        }
    }

    pub fn sharpen(&self) -> Sharpen {
        Sharpen {
            amount: self.sharpen_amount.clamp(0.0, 3.0),
            radius: self.sharpen_radius.clamp(0.3, 5.0),
        }
    }
}

fn builtin() -> Vec<Preset> {
    // Fitted to six frames of film 71 against their SilverFast scans (median error 13 % -> 2.6 %);
    // the sharpening is a first guess, the scans are softer than SilverFast's.
    vec![
        Preset::from_values(DEFAULT.into(), &Adjust::default(), &Sharpen::default()),
        Preset::from_values(
            SILVERFAST_FP4.into(),
            &Adjust {
                exposure_ev: 0.10,
                contrast: 0.20,
                darks: -0.20,
                whites: -1.0,
                gamma: 0.83,
                ..Adjust::default()
            },
            &Sharpen {
                amount: 0.5,
                radius: 1.0,
            },
        ),
    ]
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    preset: Vec<Preset>,
}

/// Built-in and user presets.
#[derive(Debug, Clone, Default)]
pub struct Presets {
    user: Vec<Preset>,
    /// Where the user presets are written; `None` = nowhere (tests).
    path: Option<PathBuf>,
}

impl Presets {
    pub fn load_from(path: &Path) -> (Self, Option<String>) {
        let mut s = Self {
            user: vec![],
            path: Some(path.to_path_buf()),
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return (s, None);
        };
        match toml::from_str::<File>(&text) {
            Ok(f) => {
                s.user = f
                    .preset
                    .into_iter()
                    .filter(|p| !p.name.is_empty())
                    .collect();
                (s, None)
            }
            Err(e) => (s, Some(format!("presets ignored: {e}"))),
        }
    }

    /// Built-in presets first, then the user's; a user preset replaces a built-in one of the
    /// same name.
    pub fn all(&self) -> Vec<&Preset> {
        let mut out: Vec<&Preset> = vec![];
        for b in builtin_static() {
            if !self.user.iter().any(|u| u.name == b.name) {
                out.push(b);
            }
        }
        out.extend(self.user.iter());
        out
    }

    pub fn get(&self, name: &str) -> Option<&Preset> {
        self.all().into_iter().find(|p| p.name == name)
    }

    /// Only user presets can be deleted (a user preset overriding a built-in one reverts it).
    pub fn can_delete(&self, name: &str) -> bool {
        self.user.iter().any(|p| p.name == name)
    }

    /// Stores `preset` (replacing one of the same name) and writes the file.
    pub fn upsert(&mut self, preset: Preset) -> Result<(), String> {
        match self.user.iter_mut().find(|p| p.name == preset.name) {
            Some(p) => *p = preset,
            None => self.user.push(preset),
        }
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        self.user.retain(|p| p.name != name);
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string_pretty(&File {
            preset: self.user.clone(),
        })
        .map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(tmp, path).map_err(|e| e.to_string())
    }
}

fn builtin_static() -> &'static [Preset] {
    static BUILTIN: std::sync::OnceLock<Vec<Preset>> = std::sync::OnceLock::new();
    BUILTIN.get_or_init(builtin)
}

/// The preset bar's state: the presets, the name being typed and the last error.
#[derive(Debug, Clone, Default)]
pub struct PresetUi {
    pub presets: Presets,
    pub name: String,
    pub error: Option<String>,
}

impl PresetUi {
    pub fn new(presets: Presets) -> Self {
        Self {
            presets,
            name: String::new(),
            error: None,
        }
    }

    /// Stores the current values under the typed name.
    /// `detail` = `None` (the scan panel has no clarity or sharpening) keeps those of an existing
    /// preset of that name.
    pub fn save_current(&mut self, a: &Adjust, detail: Option<(&Clarity, &Sharpen)>) {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            self.error = Some("type a name for the preset".into());
            return;
        }
        let (c, s) = detail
            .map(|(c, s)| (*c, *s))
            .or_else(|| self.presets.get(&name).map(|p| (p.clarity(), p.sharpen())))
            .unwrap_or_default();
        self.error = self
            .presets
            .upsert(Preset::from_values(name, a, &s).with_clarity(&c))
            .err()
            .map(|e| format!("cannot save presets: {e}"));
    }

    pub fn delete_named(&mut self) {
        let name = self.name.trim().to_string();
        self.error = self
            .presets
            .remove(&name)
            .err()
            .map(|e| format!("cannot save presets: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scan-presets-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("presets.toml")
    }

    #[test]
    fn the_builtin_preset_is_the_measured_silverfast_look() {
        let p = Presets::default();
        let b = p.get(SILVERFAST_FP4).expect("built-in");
        let a = b.adjust();
        assert_eq!(a.exposure_ev, 0.10);
        assert_eq!(a.contrast, 0.20);
        assert_eq!((a.black, a.white), (0.0, 1.0));
        assert_eq!((a.darks, a.whites), (-0.20, -1.0));
        assert_eq!(a.gamma, 0.83);
        assert!(!p.can_delete(SILVERFAST_FP4));
    }

    #[test]
    fn the_default_entry_is_neutral_first_and_permanent() {
        let p = Presets::default();
        assert_eq!(p.all()[0].name, DEFAULT);
        let d = p.get(DEFAULT).unwrap();
        assert!(d.adjust().is_neutral() && d.sharpen().is_neutral());
        assert!(!p.can_delete(DEFAULT));
    }

    #[test]
    fn round_trips_through_a_file() {
        let path = temp("rt");
        let (mut p, warn) = Presets::load_from(&path);
        assert!(warn.is_none());
        let a = Adjust {
            exposure_ev: 0.5,
            contrast: -0.3,
            ..Adjust::default()
        };
        let s = Sharpen {
            amount: 1.5,
            radius: 2.0,
        };
        p.upsert(Preset::from_values("mine".into(), &a, &s))
            .unwrap();
        let (q, warn) = Presets::load_from(&path);
        assert!(warn.is_none());
        let m = q.get("mine").unwrap();
        assert_eq!((m.adjust(), m.sharpen()), (a, s));
        assert!(q.can_delete("mine"));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_user_preset_overrides_the_builtin_of_the_same_name_and_deleting_reverts() {
        let path = temp("override");
        let (mut p, _) = Presets::load_from(&path);
        let a = Adjust {
            exposure_ev: -1.0,
            ..Adjust::default()
        };
        p.upsert(Preset::from_values(
            SILVERFAST_FP4.into(),
            &a,
            &Sharpen::default(),
        ))
        .unwrap();
        assert_eq!(
            p.all().iter().filter(|x| x.name == SILVERFAST_FP4).count(),
            1
        );
        assert_eq!(p.get(SILVERFAST_FP4).unwrap().adjust().exposure_ev, -1.0);
        p.remove(SILVERFAST_FP4).unwrap();
        assert_eq!(p.get(SILVERFAST_FP4).unwrap().adjust().exposure_ev, 0.10);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn saturation_and_clarity_are_stored_and_older_files_still_load() {
        let path = temp("sat");
        std::fs::write(&path, "[[preset]]\nname = \"old\"\nexposure_ev = 0.5\n").unwrap();
        let (mut p, warn) = Presets::load_from(&path);
        assert!(warn.is_none(), "{warn:?}");
        let old = p.get("old").unwrap();
        assert_eq!(old.adjust().saturation, 0.0);
        assert_eq!(old.clarity().amount, 0.0);

        let a = Adjust {
            saturation: -0.4,
            ..Adjust::default()
        };
        let c = Clarity { amount: 0.6 };
        p.upsert(Preset::from_values("new".into(), &a, &Sharpen::default()).with_clarity(&c))
            .unwrap();
        let (again, _) = Presets::load_from(&path);
        let new = again.get("new").unwrap();
        assert_eq!((new.adjust().saturation, new.clarity().amount), (-0.4, 0.6));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn values_are_limited_to_the_slider_ranges() {
        let p = Preset {
            name: "wild".into(),
            exposure_ev: 9.0,
            black: 0.9,
            white: 0.2,
            midtones: 10.0,
            contrast: -4.0,
            sharpen_amount: 50.0,
            sharpen_radius: 0.0,
            ..Preset::default()
        };
        let a = p.adjust();
        assert_eq!(a.exposure_ev, 2.0);
        assert_eq!(a.white, 0.6);
        assert!(a.black <= a.white - 0.1 + 1e-6);
        assert_eq!(a.gamma, 2.0);
        assert_eq!(a.contrast, -1.0);
        assert_eq!((p.sharpen().amount, p.sharpen().radius), (3.0, 0.3));
    }

    #[test]
    fn a_broken_file_gives_no_presets_and_a_warning() {
        let path = temp("broken");
        std::fs::write(&path, "preset = 5 [[").unwrap();
        let (p, warn) = Presets::load_from(&path);
        assert!(warn.is_some());
        assert!(!p.can_delete("x"));
        assert_eq!(p.all().len(), builtin_static().len());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn saving_needs_a_name() {
        let mut ui = PresetUi::default();
        ui.save_current(
            &Adjust::default(),
            Some((&Clarity::default(), &Sharpen::default())),
        );
        assert!(ui.error.is_some());
        ui.name = "  x ".into();
        ui.save_current(
            &Adjust::default(),
            Some((&Clarity::default(), &Sharpen::default())),
        );
        assert!(ui.error.is_none());
        assert!(ui.presets.can_delete("x"));
        ui.delete_named();
        assert!(!ui.presets.can_delete("x"));
    }
}
