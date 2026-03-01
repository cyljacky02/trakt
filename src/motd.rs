use std::{sync::Arc, time::Duration};

use tokio::{
    sync::{RwLock, Semaphore},
    task::JoinSet,
};

use crate::{
    config::ConfigProvider,
    raknet::ping::{self, Motd},
};

/// A controller that periodically fetches MOTD information
/// from the backend and exposes the last successful response.
pub struct MOTDReflector {
    execute_lock: Semaphore,

    /// Config provider.
    config_provider: Arc<ConfigProvider>,

    /// Last successful MOTD response, if any.
    last_motd: RwLock<Option<Motd>>,
}

impl MOTDReflector {
    pub fn new(config_provider: Arc<ConfigProvider>) -> Self {
        Self {
            execute_lock: Semaphore::new(1),
            config_provider,
            last_motd: RwLock::new(None),
        }
    }

    /// Returns a clone of the last successful MOTD information received.
    pub async fn last_motd(&self) -> Option<Motd> {
        self.last_motd.read().await.clone()
    }

    /// Fetches the MOTD concurrently from all sources, takes first success.
    pub async fn execute(&self) {
        let _permit = self.execute_lock.acquire().await;
        let (local_addr, sources, proxy_protocol) = {
            let config = self.config_provider.read().await;
            let sources = if let Some(source) = &config.backend.motd_source {
                vec![source.clone()]
            } else {
                config
                    .backend
                    .servers
                    .iter()
                    .map(|server| server.address.clone())
                    .collect()
            };
            let proxy_protocol = config.proxy_protocol.unwrap_or(true);
            (config.proxy_bind.clone(), sources, proxy_protocol)
        };
        tracing::debug!(
            source_count = sources.len(),
            "Fetching MOTD information from backend"
        );
        let timeout = Duration::from_secs(5);
        let mut join_set = JoinSet::new();
        for source in sources.into_iter() {
            let local_addr = local_addr.clone();
            join_set.spawn(async move {
                // Always ping without proxy protocol for MOTD.
                // UnconnectedPing is pre-session — Geyser doesn't need real
                // client IP, and its ping passthrough is buggy with PP enabled.
                let result = ping::ping(&local_addr, &source, false, timeout).await;
                (source, result)
            });
        }
        // Take first successful result
        while let Some(Ok((source, result))) = join_set.join_next().await {
            match result {
                Ok(motd) => {
                    tracing::debug!(%source, ?motd, "Successfully fetched MOTD information");
                    let mut w = self.last_motd.write().await;
                    *w = Some(motd);
                    return;
                }
                Err(err) => {
                    tracing::warn!(%source, ?err, "Could not fetch MOTD information");
                }
            }
        }
        tracing::warn!("All MOTD sources failed — clients will see fallback MOTD");
    }
}
