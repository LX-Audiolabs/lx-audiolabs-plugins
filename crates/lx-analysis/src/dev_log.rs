// Dev-only logging — enable with feature "dev-logging", compiles to a no-op otherwise.
// Call `init("PluginName")` once from `Plugin::initialize()` and editor::create().
// Logs go to %TEMP%/clap-dev.log, panics to %TEMP%/clap-dev-panic.log.
//
// Uses synchronous file writes so messages are never lost on crash/segfault.
// Non-blocking would lose queued messages when the process dies abruptly.

#[cfg(feature = "dev-logging")]
mod inner {
    use std::sync::{Mutex, OnceLock};

    static INIT: OnceLock<()> = OnceLock::new();

    pub fn init(plugin_name: &'static str) {
        INIT.get_or_init(|| {
            std::panic::set_hook(Box::new(move |info| {
                let path = std::env::temp_dir().join("clap-dev-panic.log");
                let msg = format!(
                    "[{}] PANIC at {:?}:\n{}\n---\n",
                    plugin_name,
                    info.location(),
                    info
                );
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    let _ = f.write_all(msg.as_bytes());
                }
            }));

            let log_path = std::env::temp_dir().join("clap-dev.log");
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .expect("clap-dev.log open failed");

            // Synchronous writer — flushes immediately on every write.
            // Critical for crash debugging: non_blocking loses queued messages on segfault.
            let _ = tracing_subscriber::fmt()
                .with_writer(Mutex::new(file))
                .with_ansi(false)
                .with_target(false)
                .with_max_level(tracing::Level::DEBUG)
                .try_init();

            tracing::info!("[{}] === dev logging started ===", plugin_name);
        });
    }
}

pub fn init(_plugin_name: &'static str) {
    #[cfg(feature = "dev-logging")]
    inner::init(_plugin_name);
}
