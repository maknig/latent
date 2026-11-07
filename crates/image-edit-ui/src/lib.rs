//! egui front end of [`latent_image_edit`]: the editing state of one image ([`editor`]), the
//! slider groups and presets, and the widgets they are made of (form rows, histogram, zoomable
//! canvas, crop overlay, dust brush). Hosts lay these out around their own window.

pub mod canvas;
pub mod crop;
pub mod dust;
pub mod edit;
pub mod editor;
pub mod form;
pub mod histogram;
pub mod presets;
pub mod preview;
pub mod sliders;
pub mod trace;
pub mod view;

use egui::Color32;

pub const ACCENT: Color32 = Color32::from_rgb(70, 130, 200);
