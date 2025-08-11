use reth_errors::ProviderError;
use reth_evm::{EvmEnv, IntoTxEnv};
use revm::context::result::InvalidTransaction;
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        TxEnv,
    },
    database::BundleState,
    Database,
};
use thiserror::Error;

pub mod evm_inspector;
use evm_inspector::RBuilderEVMInspector;

use crate::tx_sim_cache::TxStateAccessTrace;

pub mod tx_sim_cache;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TransactionErr {
    #[error("Invalid transaction: {0:?}")]
    InvalidTransaction(InvalidTransaction),
    #[error("Blocklist violation error")]
    Blocklist,
    #[error("Gas left is too low")]
    GasLeft,
    #[error("Blob Gas left is too low")]
    BlobGasLeft,
}

pub trait Evm {
    fn transact(
        &mut self,
        bundle_state: Option<BundleState>,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<ProviderError>>;
}

/// Custom trait to abstract over EVM construction with a cleaner and more concrete
/// interface than the `Evm` trait from `alloy-revm`.
///
/// # Motivation
///
/// The `alloy_revm::Evm` trait comes with a large number of associated types and trait
/// bounds. This new `EvmFactory` trait is designed to encapsulate those complexities,
/// providing an EVM interface less dependent on `alloy-revm` crate.
///
/// It is particularly useful in reducing trait bound noise in other parts of the codebase
/// (i.e. `execute_evm` in `order_commit`), and improves modularity.
///
/// See [`EthCachedEvmFactory`] for an implementation that integrates precompile
/// caching and uses `reth_evm::EthEvm` internally.
pub trait EvmFactory {
    /// Create an EVM instance without any tracing
    fn create_evm<DB>(&self, db: DB, env: EvmEnv) -> impl Evm
    where
        DB: Database<Error = ProviderError>;

    /// Create an EVM instance with tracers (state access recording, used state tracing inspector, access list inspector)
    fn create_evm_with_tracers<DB>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: &mut RBuilderEVMInspector,
        recorded_state_access_trace: Option<&mut TxStateAccessTrace>,
    ) -> impl Evm
    where
        DB: Database<Error = ProviderError>;
}

mod revm_evm;

pub type RBuilderEvm = revm_evm::EthCachedEvmFactory;
