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
                debug!("Config change detected, persisting to disk");

                // The lock keeps this write from interleaving with `wayle config set`, and
                // `save_if_changed` keeps reloads of other writers' changes from being
                // written back as a stale snapshot.
                let _lock = match ConfigService::lock_runtime().await {
                    Ok(lock) => Some(lock),
                    Err(e) => {
                        error!(error = %e, "cannot lock runtime.toml, persisting without it");
                        None
                    }
                };

                if let Err(e) = service.save_if_changed().await {
                    error!(error = %e, "cannot persist config changes");
                }
            }
        });

        Ok(Self)
    }
}
