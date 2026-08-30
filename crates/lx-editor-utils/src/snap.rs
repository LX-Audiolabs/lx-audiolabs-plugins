//! SNAP file naming and background preset-vault scanning utilities.

use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

/// Find the next free `SNAPSHOT-NNN.md` name in `vault_path`.
pub fn snap_filename(vault_path: &str) -> String {
    let dir = Path::new(vault_path);
    let mut max_n = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let s = e.file_name().to_string_lossy().into_owned();
            if let Some(inner) = s
                .strip_prefix("SNAPSHOT-")
                .and_then(|r| r.strip_suffix(".md"))
                && let Ok(n) = inner.parse::<u32>()
            {
                max_n = max_n.max(n);
            }
        }
    }
    format!("SNAPSHOT-{:03}.md", max_n + 1)
}

/// Thread-safe mailbox for background preset vault scans.
///
/// `T` is the plugin-specific preset entry type.
pub struct PendingPresets<T> {
    pub ready: AtomicBool,
    pub generation: AtomicU32,
    pub presets: Mutex<Option<(u32, Vec<T>)>>,
}

impl<T> PendingPresets<T> {
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            presets: Mutex::new(None),
        }
    }

    /// Increment generation, mark pending as not ready, and clear any stale result.
    pub fn bump_generation(&self) -> u32 {
        let new = self.generation.load(Ordering::Relaxed).wrapping_add(1);
        self.generation.store(new, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        if let Ok(mut guard) = self.presets.lock() {
            *guard = None;
        }
        new
    }
}

impl<T> Default for PendingPresets<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn a background thread that scans the vault and stores the result.
///
/// `scan` is the plugin-specific closure that returns the preset list. It runs
/// in a new thread, so all data it captures must be `Send + 'static`.
pub fn spawn_vault_scan<T, S>(pending: Arc<PendingPresets<T>>, generation: u32, scan: S)
where
    T: Send + 'static,
    S: FnOnce() -> Vec<T> + Send + 'static,
{
    std::thread::spawn(move || {
        let scanned = scan();
        if let Ok(mut guard) = pending.presets.lock() {
            *guard = Some((generation, scanned));
        }
        pending.ready.store(true, Ordering::Release);
    });
}
