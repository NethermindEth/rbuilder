use serde::Deserialize;
use std::{fmt::Debug, path::PathBuf};

/// After how many milliseconds should we give up on an IPC request (consider it failed)
/// 100ms was picked up after initial testing using Nethermind client as state provider
/// 99.9% requests return within 50ms; using 100ms gives us error rate of ~0.03%
/// Median response time is ~300 micro_sec.
pub(crate) const DEFAULT_IPC_REQUEST_TIMEOUT_MS: u64 = 100;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct IpcProviderConfig {
    pub(crate) request_timeout_ms: u64,
    pub(crate) ipc_path: PathBuf,
    pub(crate) mempool_server_url: String,
}

impl Default for IpcProviderConfig {
    fn default() -> Self {
        Self {
            request_timeout_ms: DEFAULT_IPC_REQUEST_TIMEOUT_MS,
            mempool_server_url: String::new(),
            ipc_path: PathBuf::new(),
        }
    }
}
