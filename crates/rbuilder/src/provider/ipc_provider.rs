use alloy_json_rpc::RpcSend;
use reipc::rpc_provider::RpcProvider;
use reth_errors::{ProviderError, ProviderResult};
use reth_provider::errors::any::AnyError;
use serde::{de::DeserializeOwned, Deserialize};
use std::{borrow::Cow, fmt::Debug, path::PathBuf};
use tracing::{trace, trace_span};

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

pub(crate) fn rpc_call<Param, Resp>(
    ipc_provider: &RpcProvider,
    rpc_method: impl Into<Cow<'static, str>> + tracing::Value,
    params: Param,
) -> ProviderResult<Resp>
where
    Param: RpcSend,
    Resp: DeserializeOwned + derive_more::with_trait::Debug,
{
    let span = trace_span!("rpc_call", rpc_method, id = rand::random::<u64>());
    let _guard = span.enter();
    trace!("send request");

    let resp = ipc_provider
        .call::<Param, Resp>(rpc_method, params)
        .map_err(ipc_to_provider_error);

    trace!("response received");
    resp
}

fn ipc_to_provider_error(e: reipc::errors::RpcError) -> ProviderError {
    ProviderError::Other(AnyError::new(e))
}
