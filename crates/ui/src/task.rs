//! What the status bar shows for a running background job.
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct TaskInfo {
    pub label: &'static str,
    /// (done, total); `None` when the job cannot say how far it is.
    pub progress: Option<(usize, usize)>,
    /// Setting this asks the job to stop.
    pub cancel: Arc<AtomicBool>,
}
