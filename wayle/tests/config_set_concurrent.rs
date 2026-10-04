//! Concurrent `wayle config set` runs must not lose each other's values or corrupt
//! `runtime.toml`.
//!
//! Each invocation loads `runtime.toml`, changes one key and writes the whole file back, so
//! without cross-process locking the last writer silently drops every other writer's key, and
//! a shared temp file can be interleaved into invalid TOML.

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

const SETTINGS: &[(&str, &str)] = &[
    ("general.font-sans", "\"Sans A\""),
    ("general.font-mono", "\"Mono B\""),
    ("styling.scale", "1.1"),
    ("bar.scale", "0.9"),
    ("bar.inset-edge", "0.7"),
    ("bar.inset-ends", "0.8"),
    ("bar.module-gap", "0.5"),
    ("bar.dropdown-opacity", "42"),
];

fn scratch_dir(name: &str) -> io::Result<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn spawn_set(home: &Path, path: &str, value: &str) -> io::Result<Child> {
    Command::new(env!("CARGO_BIN_EXE_wayle"))
        .args(["config", "set", path, value])
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
}

#[test]
fn concurrent_config_set_keeps_every_value() -> Result<(), Box<dyn std::error::Error>> {
    // Racy by nature: repeat so a missing lock is caught reliably, not by luck.
    for round in 0..5 {
        let home = scratch_dir(&format!("config-set-concurrent-{round}"))?;

        let children = SETTINGS
            .iter()
            .map(|(path, value)| spawn_set(&home, path, value))
            .collect::<Result<Vec<_>, _>>()?;

        for child in children {
            let output = child.wait_with_output()?;
            assert!(
                output.status.success(),
                "a `wayle config set` failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let runtime = fs::read_to_string(home.join("config/wayle/runtime.toml"))?;
        let parsed: toml::Table = toml::from_str(&runtime).map_err(|err| {
            format!("round {round}: runtime.toml is not valid TOML: {err}\n{runtime}")
        })?;

        let missing: Vec<&str> = SETTINGS
            .iter()
            .map(|(path, _)| *path)
            .filter(|path| {
                let mut node = Some(&toml::Value::Table(parsed.clone()));
                for segment in path.split('.') {
                    node = node.and_then(|value| value.get(segment));
                }
                node.is_none()
            })
            .collect();

        assert!(
            missing.is_empty(),
            "round {round}: lost updates for {missing:?}\n{runtime}"
        );
    }
    Ok(())
}
