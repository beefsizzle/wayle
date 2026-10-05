//! Wayle desktop shell - a GTK4/Relm4 status bar for Wayland compositors.

use std::{env, error::Error, path::PathBuf};

use relm4::RelmApp;
use tokio::runtime::{Builder, Runtime};
use tracing::info;

mod bootstrap;
mod glob;
mod i18n;
mod process;
mod services;
mod shell;
mod startup;
mod systemd;
mod template;
mod tracing_init;
mod wallpaper_map;
mod watchers;

use shell::{Shell, ShellInit};

/// Worker threads for the shell's tokio runtime.
///
/// Tokio defaults to one worker per core (16 on a typical desktop), but the shell's async work is
/// D-Bus traffic and file watching while GTK owns the main thread. Override with
/// `TOKIO_WORKER_THREADS`.
const DEFAULT_WORKER_THREADS: usize = 4;

fn build_runtime() -> std::io::Result<Runtime> {
    let workers = env::var("TOKIO_WORKER_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|workers| *workers > 0)
        .unwrap_or(DEFAULT_WORKER_THREADS);

    Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
}

/// Caps glibc's per-thread malloc arenas at two.
///
/// Every thread that allocates gets its own arena (up to eight per core). With the GL renderer
/// each popover's GPU-driver helper threads claimed their own, and the fragmentation they left
/// added ~45 MB of resident memory once every dropdown had been opened (305 MB -> 260 MB in the
/// nested harness). A `MALLOC_ARENA_MAX` set by the user wins.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn limit_malloc_arenas() {
    if env::var_os("MALLOC_ARENA_MAX").is_some() {
        return;
    }

    #[allow(unsafe_code)]
    // SAFETY: `mallopt` only changes an allocator tunable, and this runs first thing in `run`,
    // before the process has started any other thread.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn limit_malloc_arenas() {}

/// Launches the Wayle shell GUI.
///
/// Creates its own tokio runtime internally, so this must not be called
/// from within an existing tokio context (it will panic).
///
/// `config_override` points the main config file at a path other than the
/// default `~/.config/wayle/config.toml`; other config files are unaffected.
///
/// # Errors
///
/// Returns error on tracing init failure, runtime creation failure,
/// or service bootstrap failure.
pub fn run(config_override: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    limit_malloc_arenas();

    if env::var_os("GSK_RENDERER").is_none() {
        #[allow(unsafe_code)]
        // SAFETY: single-threaded, called before any runtime or GTK init
        unsafe {
            env::set_var("GSK_RENDERER", "gl");
        }
    }

    tracing_init::init()?;
    info!("Wayle shell starting");

    let runtime = build_runtime()?;
    let _guard = runtime.enter();

    if runtime.block_on(bootstrap::is_already_running()) {
        eprintln!("Wayle shell is already running");
        return Ok(());
    }

    let (timer, services) = runtime.block_on(bootstrap::init_services(config_override))?;
    info!("Services initialized");

    let app = RelmApp::new("com.wayle.shell")
        .visible_on_activate(false)
        .with_args(vec![]);

    app.run::<Shell>(ShellInit { timer, services });

    info!("Wayle shell stopped");

    runtime.shutdown_background();
    Ok(())
}
