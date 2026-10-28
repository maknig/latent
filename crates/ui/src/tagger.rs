//! "Auto tags": runs `latent_vision::suggest_tags` for one image on a background thread.
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

enum Msg {
    Done(String, Vec<(String, f32)>),
    Error(String),
}

pub struct Tagger {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub running: bool,
    /// Image the suggestions belong to, with (tag, score).
    pub result: Option<(String, Vec<(String, f32)>)>,
    pub error: Option<String>,
}

impl Default for Tagger {
    fn default() -> Self {
        let (tx, rx) = channel();
        Tagger {
            tx,
            rx,
            running: false,
            result: None,
            error: None,
        }
    }
}

impl Tagger {
    pub fn start(
        &mut self,
        ctx: &egui::Context,
        base: PathBuf,
        query: String,
        tagged: Vec<(String, Vec<String>)>,
    ) {
        if self.running {
            return;
        }
        self.running = true;
        self.error = None;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let msg = match latent_vision::suggest_tags(&base, &query, &tagged) {
                Ok(r) => Msg::Done(query, r),
                Err(e) => Msg::Error(e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            self.running = false;
            match m {
                Msg::Done(q, r) => self.result = Some((q, r)),
                Msg::Error(e) => self.error = Some(e),
            }
        }
    }
}
