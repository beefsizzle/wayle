//! Verifies a removed niri-workspaces key warns at load instead of being silently ignored.

use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use tracing_subscriber::fmt::MakeWriter;
use wayle_config::{ApplyConfigLayer, CommitConfigReload, Config};

#[derive(Clone, Default)]
struct CapturedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedBuffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut guard = match self.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn stale_min_workspace_count_warns_and_has_no_effect() -> Result<(), Box<dyn std::error::Error>> {
    let writer = CapturedBuffer::default();
    let captured = Arc::clone(&writer.0);

    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .without_time()
        .finish();

    let toml_input: toml::Value = toml::from_str(
        r#"
        [modules.niri-workspaces]
        min-workspace-count = 5
        hide-trailing-empty = false
        "#,
    )?;

    let hide_trailing_empty = tracing::subscriber::with_default(subscriber, || {
        let config = Config::default();
        config.apply_config_layer(&toml_input, "");
        config.commit_config_reload();
        config.modules.niri_workspaces.hide_trailing_empty.get()
    });

    let bytes = match captured.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let log = String::from_utf8(bytes)?;

    assert!(
        log.contains("removed config key has no effect"),
        "expected removed-key warning, got: {log}"
    );
    assert!(
        log.contains("modules.niri-workspaces.min-workspace-count"),
        "expected the full key path, got: {log}"
    );
    assert!(
        !hide_trailing_empty,
        "other keys in the same table must still apply"
    );
    Ok(())
}
