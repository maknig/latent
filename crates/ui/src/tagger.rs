//! "Auto tags": runs `latent_vision::suggest_tags` for one image on a background thread.
use crate::task::TaskInfo;
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

enum Msg {
    Progress(usize, usize),
    Done(String, Vec<(String, f32)>),
    Error(String),
    Cancelled,
}

pub struct Tagger {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub running: bool,
    progress: Option<(usize, usize)>,
    cancel: Arc<AtomicBool>,
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
            progress: None,
            cancel: Default::default(),
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
        self.progress = None;
        self.error = None;
        self.cancel.store(false, Ordering::Relaxed);
        let (tx, ctx, cancel) = (self.tx.clone(), ctx.clone(), self.cancel.clone());
        std::thread::spawn(move || {
            let report = |d, t| {
                let _ = tx.send(Msg::Progress(d, t));
                ctx.request_repaint();
            };
            let msg = match latent_vision::suggest_tags(&base, &query, &tagged, &cancel, &report) {
                Ok(r) => Msg::Done(query, r),
                Err(e) if e == latent_vision::CANCELLED => Msg::Cancelled,
                Err(e) => Msg::Error(e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            if let Msg::Progress(d, t) = m {
                self.progress = Some((d, t));
                continue;
            }
            self.running = false;
            self.progress = None;
            match m {
                Msg::Done(q, r) => self.result = Some((q, r)),
                Msg::Error(e) => self.error = Some(e),
                Msg::Progress(..) | Msg::Cancelled => {}
            }
        }
    }

    pub fn task(&self) -> Option<TaskInfo> {
        self.running.then(|| TaskInfo {
            label: "Suggesting tags",
            progress: self.progress,
            cancel: self.cancel.clone(),
        })
    }
}
