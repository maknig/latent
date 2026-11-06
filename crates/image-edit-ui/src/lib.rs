//! egui front end of [`latent_image_edit`]: the editing state of one image ([`editor`]), the
//! slider groups and presets, and the widgets they are made of (form rows, histogram). Hosts
//! draw the picture, the crop frame and the dust brush themselves.

pub mod edit;
pub mod editor;
pub mod form;
pub mod histogram;
pub mod presets;
pub mod preview;
pub mod sliders;
pub mod trace;

use egui::Color32;

pub const ACCENT: Color32 = Color32::from_rgb(70, 130, 200);
