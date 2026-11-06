//! Slow-step tracing: with the environment variable `LATENT_SCAN_TRACE` set, steps that take
//! long enough to make a window feel stuck are reported on stderr.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Steps shorter than this are not reported.
const SLOW: Duration = Duration::from_millis(100);

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LATENT_SCAN_TRACE").is_some_and(|v| !v.is_empty()))
}

/// Runs `f` and reports it when tracing is on and it was slow.
pub fn timed<R>(label: &str, f: impl FnOnce() -> R) -> R {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    let took = start.elapsed();
    if took >= SLOW {
        eprintln!("[trace] {label}: {} ms", took.as_millis());
    }
    result
}
