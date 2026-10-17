//! Background thumbnail loading and texture cache (native only).
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

pub enum Thumb {
    Loading,
    Ready(egui::TextureHandle),
    Missing,
}

type Key = (PathBuf, u32);

pub struct Thumbs {
    map: HashMap<Key, Thumb>,
    tx: Sender<(Key, Option<egui::ColorImage>)>,
    rx: Receiver<(Key, Option<egui::ColorImage>)>,
}

impl Default for Thumbs {
    fn default() -> Self {
        let (tx, rx) = channel();
        Thumbs { map: HashMap::new(), tx, rx }
    }
}

impl Thumbs {
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, img)) = self.rx.try_recv() {
            let t = match img {
                Some(ci) => Thumb::Ready(ctx.load_texture(key.0.to_string_lossy(), ci, Default::default())),
                None => Thumb::Missing,
            };
            self.map.insert(key, t);
        }
    }

    /// Texture of `path` scaled to fit `size` px; starts a background decode on first use.
    pub fn get(&mut self, ctx: &egui::Context, path: &Path, size: u32) -> &Thumb {
        let key = (path.to_path_buf(), size);
        if !self.map.contains_key(&key) {
            self.map.insert(key.clone(), Thumb::Loading);
            let (tx, ctx, key) = (self.tx.clone(), ctx.clone(), key.clone());
            std::thread::spawn(move || {
                let ci = image::open(&key.0).ok().map(|im| {
                    let im = im.thumbnail(key.1, key.1).to_rgba8();
                    let size = [im.width() as usize, im.height() as usize];
                    egui::ColorImage::from_rgba_unmultiplied(size, im.as_raw())
                });
                let _ = tx.send((key, ci));
                ctx.request_repaint();
            });
        }
        &self.map[&key]
    }
}
