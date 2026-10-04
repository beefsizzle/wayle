use std::{
    fs::TryLockError,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};
use tracing::{info, instrument, warn};

use super::{
    error::{Error, InvalidFieldReason, IoOperation},
    paths::ConfigPaths,
    secrets, toml_path,
    watcher::FileWatcher,
};
use crate::{
    ApplyConfigLayer, ApplyRuntimeLayer, ClearAllRuntime, ClearRuntimeByPath, CommitConfigReload,
    Config, ExtractRuntimeValues, infrastructure::themes::utils::load_themes,
};

/// Distinguishes temp files written by concurrent saves within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// How long to wait for another process to release the runtime lock before giving up.
const RUNTIME_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const RUNTIME_LOCK_POLL: Duration = Duration::from_millis(25);
const RUNTIME_LOCK_NOTICE_AFTER: Duration = Duration::from_secs(1);

/// Exclusive, cross-process lock over `runtime.toml`'s read-modify-write cycle.
///
/// Held from before the config is loaded until after it is saved, so concurrent writers (for
/// example several `wayle config set` runs) apply one after another instead of each saving a
/// snapshot that is missing the others' changes. Released on drop.
///
/// Not reentrant: acquiring it again while holding it, even from the same process, waits for
/// the first guard and times out.
#[derive(Debug)]
pub struct RuntimeLock {
    _file: std::fs::File,
}

impl RuntimeLock {
    /// Blocks (up to `timeout`) until the lock at `path` is free, then takes it.
    ///
    /// If the filesystem cannot lock at all, warns and carries on unlocked: the old
    /// unserialized behaviour beats refusing to write settings.
    fn acquire(path: &Path, timeout: Duration) -> Result<Self, Error> {
        let persistence = |source| Error::Persistence {
            path: path.to_path_buf(),
            source,
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(persistence)?;
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(persistence)?;

        let started = Instant::now();
        let mut announced = false;

        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) => {
                    let waited = started.elapsed();

                    if waited >= timeout {
                        return Err(persistence(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out waiting for another wayle process to release the \
                             runtime.toml lock",
                        )));
                    }

                    if !announced && waited >= RUNTIME_LOCK_NOTICE_AFTER {
                        warn!(
                            path = %path.display(),
                            "waiting for another wayle process to finish writing runtime.toml"
                        );
                        announced = true;
                    }

                    std::thread::sleep(RUNTIME_LOCK_POLL);
                }
                Err(TryLockError::Error(err)) => {
                    warn!(
                        error = %err,
                        path = %path.display(),
                        "cannot lock runtime.toml; continuing without the lock"
                    );
                    return Ok(Self { _file: file });
                }
            }
        }
    }
}

/// Reactive configuration service.
///
/// Each config field can be watched independently for changes. Runtime
/// overrides are extracted directly from `ConfigProperty` fields.
#[derive(Clone)]
pub struct ConfigService {
    config: Arc<Config>,
    watcher: Arc<RwLock<Option<FileWatcher>>>,
    main_config_path: Arc<PathBuf>,
    /// Runtime values as of the last load, reload or save, i.e. what this process believes
    /// `runtime.toml` holds. Lets background saves skip when nothing changed locally, so a
    /// reload never echoes a stale snapshot over another writer's newer file.
    synced_runtime: Arc<Mutex<Option<toml::Value>>>,
}

impl ConfigService {
    /// Loads configuration from TOML files and starts file watcher.
    ///
    /// Applies `config.toml` (or `config_override`, if given) to the config
    /// layer and `runtime.toml` to the runtime layer, then starts hot-reload
    /// file watching. `config_override` changes only the main config file path.
    ///
    /// # Errors
    ///
    /// Returns error if config files cannot be loaded or parsed.
    #[instrument]
    pub async fn load(config_override: Option<PathBuf>) -> Result<Arc<Self>, Error> {
        info!("Loading configuration");

        if let Ok(config_dir) = ConfigPaths::config_dir() {
            secrets::load_env_files(&config_dir);
        }

        let config = Config::default();
        let main_config_path = config_override.unwrap_or_else(ConfigPaths::main_config);

        let config_path = main_config_path.clone();
        let config_result =
            tokio::task::spawn_blocking(move || Config::load_toml_with_imports(&config_path))
                .await
                .map_err(|source| Error::TaskJoin { source })?;

        match config_result {
            Ok(config_toml) => {
                // Removed/renamed keys still present are warned about by the derived
                // `apply_config_layer` (from `#[wayle(deprecated(...))]` /
                // `#[wayle(deprecated_alias)]` on the schema fields themselves).
                config.apply_config_layer(&config_toml, "");
            }
            Err(e) => warn!("using defaults, config.toml failed:\n{e}"),
        }

        let runtime_path = ConfigPaths::runtime_config();
        let runtime_result =
            tokio::task::spawn_blocking(move || Self::load_toml_file(&runtime_path))
                .await
                .map_err(|source| Error::TaskJoin { source })?;

        match runtime_result {
            Ok(runtime_toml) => {
                if let Err(e) = config.apply_runtime_layer(&runtime_toml, "") {
                    warn!("invalid runtime.toml value:\n{e}");
                }
            }
            Err(e) => warn!("runtime.toml failed:\n{e}"),
        }

        config.commit_config_reload();

        let service = Arc::new(Self {
            config: Arc::new(config),
            watcher: Arc::new(RwLock::new(None)),
            main_config_path: Arc::new(main_config_path),
            synced_runtime: Arc::new(Mutex::new(None)),
        });
        service.record_synced();

        let themes_dir = ConfigPaths::themes_dir();
        load_themes(&service.config, &themes_dir);

        let file_watcher = FileWatcher::start(Arc::clone(&service))?;
        *service
            .watcher
            .write()
            .map_err(|_| Error::WatcherPoisoned)? = Some(file_watcher);

        info!("Configuration loaded successfully");

        Ok(service)
    }

    /// Path of the lock file guarding `runtime.toml` read-modify-write cycles.
    pub fn runtime_lock_path() -> PathBuf {
        ConfigPaths::runtime_config().with_extension("lock")
    }

    /// Takes the exclusive lock over `runtime.toml`, waiting for other holders.
    ///
    /// Hold the returned guard across `load` and `save` whenever the result of a load is
    /// written back, so no other writer's change is overwritten.
    ///
    /// # Errors
    ///
    /// Returns error if the lock file cannot be created or locked.
    pub async fn lock_runtime() -> Result<RuntimeLock, Error> {
        let lock_path = Self::runtime_lock_path();

        tokio::task::spawn_blocking(move || RuntimeLock::acquire(&lock_path, RUNTIME_LOCK_TIMEOUT))
            .await
            .map_err(|source| Error::TaskJoin { source })?
    }

    /// Records the current runtime values as what `runtime.toml` holds.
    pub(crate) fn record_synced(&self) {
        let current = self.config.extract_runtime_values();

        if let Ok(mut synced) = self.synced_runtime.lock() {
            *synced = current;
        }
    }

    /// Saves like [`save`](Self::save), but only if the runtime values differ from what was
    /// last loaded or saved. Returns whether it wrote.
    ///
    /// For background persistence, whose triggers include reloads of changes another process
    /// made: those leave the values equal to disk, and writing them back would only risk
    /// overwriting an even newer file.
    ///
    /// # Errors
    ///
    /// Returns error if config cannot be serialized or written to disk.
    pub async fn save_if_changed(&self) -> Result<bool, Error> {
        let current = self.config.extract_runtime_values();
        let unchanged = self
            .synced_runtime
            .lock()
            .is_ok_and(|synced| *synced == current);

        if unchanged {
            return Ok(false);
        }

        self.save().await?;
        Ok(true)
    }

    /// Reference to the config root.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Path to the effective main config file (the `--config` override if one
    /// was given, otherwise the XDG default).
    pub fn main_config_path(&self) -> &Path {
        &self.main_config_path
    }

    /// Drops every runtime override and deletes `runtime.toml` from disk.
    ///
    /// The in-memory reset happens first so subscribers see fresh values
    /// immediately. The file removal is best-effort: if it fails (permission,
    /// concurrent removal), a warning is logged and the error is returned so
    /// callers can decide whether to surface it to the user.
    ///
    /// # Errors
    ///
    /// Returns an error if `runtime.toml` exists but cannot be removed.
    pub fn reset_all_runtime(&self) -> Result<(), io::Error> {
        // Held until the file is gone, so a concurrent `config set` cannot write back
        // overrides that were loaded before this reset.
        let _lock = RuntimeLock::acquire(&Self::runtime_lock_path(), RUNTIME_LOCK_TIMEOUT)
            .map_err(|err| {
                warn!(error = %err, "resetting runtime.toml without its lock");
            })
            .ok();

        self.config.clear_all_runtime();
        self.record_synced();

        let runtime_path = ConfigPaths::runtime_config();

        match std::fs::remove_file(&runtime_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %runtime_path.display(),
                    "failed to remove runtime.toml"
                );
                Err(err)
            }
        }
    }

    /// Subscribes to secrets reload events.
    ///
    /// Returns a receiver that fires whenever `.env` files are reloaded.
    /// Returns `None` if the watcher is not initialized.
    pub fn subscribe_secrets_reload(&self) -> Option<tokio::sync::watch::Receiver<()>> {
        self.watcher.read().ok().and_then(|guard| {
            guard
                .as_ref()
                .map(|watcher| watcher.subscribe_secrets_reload())
        })
    }

    /// Subscribes to config reload events.
    ///
    /// Returns a receiver that fires whenever the main or runtime config is reloaded
    /// from disk (after the new values are committed). Returns `None` if the watcher
    /// is not initialized.
    pub fn subscribe_config_reload(&self) -> Option<tokio::sync::watch::Receiver<()>> {
        self.watcher.read().ok().and_then(|guard| {
            guard
                .as_ref()
                .map(|watcher| watcher.subscribe_config_reload())
        })
    }

    /// Persists runtime layer values to `runtime.toml`.
    ///
    /// Only values with runtime overrides are written.
    ///
    /// # Errors
    ///
    /// Returns error if config cannot be serialized or written to disk.
    #[instrument(skip(self))]
    pub async fn save(&self) -> Result<(), Error> {
        let runtime_value = self
            .config
            .extract_runtime_values()
            .unwrap_or_else(|| toml::Value::Table(toml::Table::new()));

        let runtime_path = ConfigPaths::runtime_config();
        let temp_path = Self::unique_temp_path(&runtime_path);

        let toml_str =
            toml::to_string_pretty(&runtime_value).map_err(|source| Error::Serialization {
                content_type: "runtime config",
                source,
            })?;

        if let Err(err) = Self::write_and_replace(&temp_path, &runtime_path, &toml_str).await {
            let _ = fs::remove_file(&temp_path).await;
            return Err(err);
        }

        let dir_path = runtime_path.parent().ok_or_else(|| Error::Persistence {
            path: runtime_path.clone(),
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime config path has no parent directory",
            ),
        })?;

        File::open(dir_path)
            .await
            .map_err(|source| Error::Persistence {
                path: dir_path.to_path_buf(),
                source,
            })?
            .sync_all()
            .await
            .map_err(|source| Error::Persistence {
                path: dir_path.to_path_buf(),
                source,
            })?;

        self.record_synced();
        info!("Configuration saved to runtime.toml");

        Ok(())
    }

    /// A temp path next to `runtime_path` that no other writer, in this or another process, uses.
    pub(crate) fn unique_temp_path(runtime_path: &Path) -> PathBuf {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        runtime_path.with_extension(format!("{}-{sequence}.tmp", std::process::id()))
    }

    async fn write_and_replace(
        temp_path: &Path,
        runtime_path: &Path,
        contents: &str,
    ) -> Result<(), Error> {
        let persistence = |path: &Path| {
            let path = path.to_path_buf();
            move |source| Error::Persistence { path, source }
        };

        let mut file = File::create(temp_path)
            .await
            .map_err(persistence(temp_path))?;

        file.write_all(contents.as_bytes())
            .await
            .map_err(persistence(temp_path))?;

        file.sync_all().await.map_err(persistence(temp_path))?;
        drop(file);

        fs::rename(temp_path, runtime_path)
            .await
            .map_err(persistence(runtime_path))
    }

    pub(crate) fn load_toml_file(path: &std::path::Path) -> Result<toml::Value, Error> {
        let content = std::fs::read_to_string(path).map_err(|source| Error::Io {
            operation: IoOperation::ReadFile,
            path: path.to_path_buf(),
            source,
        })?;

        toml::from_str(&content).map_err(|source| Error::TomlParse {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// CLI extension for string-based config access.
///
/// Application code should prefer strongly-typed access via `config()`.
pub trait ConfigServiceCli {
    /// Retrieves value at a dot-separated path (e.g., `battery.enabled`).
    ///
    /// # Errors
    ///
    /// Returns error if path is invalid.
    fn get_by_path(&self, path: &str) -> Result<toml::Value, Error>;

    /// Sets a runtime override at a dot-separated path.
    ///
    /// # Errors
    ///
    /// Returns error if path is invalid.
    fn set_by_path(&self, path: &str, value: toml::Value) -> Result<(), Error>;

    /// Clears the runtime override at a dot-separated path.
    ///
    /// Returns `true` if a value was cleared, `false` if no override existed.
    ///
    /// # Errors
    ///
    /// Returns error if path is invalid.
    fn reset_by_path(&self, path: &str) -> Result<bool, Error>;
}

impl ConfigServiceCli for ConfigService {
    fn get_by_path(&self, path: &str) -> Result<toml::Value, Error> {
        let config_value =
            toml::Value::try_from(self.config.as_ref()).map_err(|source| Error::Serialization {
                content_type: "config",
                source,
            })?;

        let mut value = config_value;
        for segment in path.split('.') {
            value = value
                .get(segment)
                .ok_or_else(|| Error::InvalidConfigField {
                    field: segment.to_string(),
                    component: path.to_string(),
                    reason: InvalidFieldReason::NotFound,
                })?
                .clone();
        }

        Ok(value)
    }

    fn set_by_path(&self, path: &str, value: toml::Value) -> Result<(), Error> {
        let mut root = toml::Value::Table(toml::Table::new());
        toml_path::insert(&mut root, path, value)?;
        self.config
            .apply_runtime_layer(&root, "")
            .map_err(Error::InvalidValue)
    }

    fn reset_by_path(&self, path: &str) -> Result<bool, Error> {
        self.config
            .clear_runtime_by_path(path)
            .map_err(Error::InvalidValue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_lock_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wayle-lock-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join("runtime.lock")
    }

    #[test]
    fn lock_times_out_while_another_guard_holds_it() {
        let path = scratch_lock_path("timeout");
        let _held = RuntimeLock::acquire(&path, Duration::from_secs(5)).unwrap();

        let started = Instant::now();
        let second = RuntimeLock::acquire(&path, Duration::from_millis(200));

        let kind = match second {
            Err(Error::Persistence { source, .. }) => Some(source.kind()),
            _ => None,
        };
        assert_eq!(kind, Some(io::ErrorKind::TimedOut));
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn lock_is_free_again_after_the_guard_drops() {
        let path = scratch_lock_path("release");

        drop(RuntimeLock::acquire(&path, Duration::from_secs(5)).unwrap());

        assert!(RuntimeLock::acquire(&path, Duration::from_millis(200)).is_ok());
    }

    #[test]
    fn lock_waits_for_a_holder_that_releases_in_time() {
        let path = scratch_lock_path("handoff");
        let held = RuntimeLock::acquire(&path, Duration::from_secs(5)).unwrap();

        let waiter = {
            let path = path.clone();
            std::thread::spawn(move || RuntimeLock::acquire(&path, Duration::from_secs(5)).is_ok())
        };

        std::thread::sleep(Duration::from_millis(100));
        drop(held);

        assert!(waiter.join().unwrap());
    }

    #[tokio::test]
    async fn save_if_changed_skips_when_nothing_differs_from_the_synced_values() {
        let service = ConfigService {
            config: Arc::new(Config::default()),
            watcher: Arc::new(RwLock::new(None)),
            main_config_path: Arc::new(PathBuf::from("unused.toml")),
            synced_runtime: Arc::new(Mutex::new(None)),
        };

        // A default config has no runtime values and nothing was synced, so there is nothing
        // to write (and this must not touch the real runtime.toml).
        assert!(!service.save_if_changed().await.unwrap());
    }
}
