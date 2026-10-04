use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info, instrument};

use super::{error::Error, service::ConfigService};
use crate::SubscribeChanges;

/// Auto-saves configuration changes to disk.
///
/// Monitors all `ConfigProperty` fields and persists changes when
/// any property is modified.
pub struct PersistenceWatcher;

impl PersistenceWatcher {
    /// Starts watching for config changes and auto-saving to disk.
    ///
    /// # Errors
    ///
    /// Returns error if persistence cannot be initialized.
    #[instrument(skip(config_service))]
    pub fn start(config_service: Arc<ConfigService>) -> Result<Self, Error> {
        let (tx, mut rx) = mpsc::unbounded_channel();

        config_service.config().subscribe_changes(tx);

        info!("Config persistence watcher started");

        let service = Arc::clone(&config_service);
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                // One save covers every change queued so far, since it reads the latest
                // state. Draining keeps a burst, or a long wait for the lock, to one save.
                while rx.try_recv().is_ok() {}

                debug!("Config change detected, persisting to disk");

                // The lock keeps this write from overlapping `wayle config set`, and
                // `save_if_changed` keeps reloads of other writers' changes from being
                // written back. It cannot make this process's in-memory copy fresher: a
                // local edit made before a just-written CLI change has been reloaded is
                // saved on top of that stale copy (the reload follows within ~100ms).
                let _lock = match ConfigService::lock_runtime().await {
                    Ok(lock) => Some(lock),
                    Err(e) => {
                        error!(error = %e, "cannot lock runtime.toml, persisting without it");
                        None
                    }
                };

                while rx.try_recv().is_ok() {}

                if let Err(e) = service.save_if_changed().await {
                    error!(error = %e, "cannot persist config changes");
                }
            }
        });

        Ok(Self)
    }
}
