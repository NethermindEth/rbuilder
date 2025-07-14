use reth_evm::{eth::EthEvmContext, EvmEnv, IntoTxEnv};
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        TxEnv,
    },
    interpreter::interpreter::EthInterpreter,
    Database, Inspector,
};

pub trait Evm<DB: Database> {
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>>;
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
    /// Create an EVM instance with default (no-op) inspector.
    fn create_evm<DB>(&self, db: DB, env: EvmEnv) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>;

    /// Create an EVM instance with a provided inspector.
    fn create_evm_with_inspector<DB, I>(&self, db: DB, env: EvmEnv, inspector: I) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<EthEvmContext<DB>, EthInterpreter>;
}

mod nethermind;
mod revm_evm;

// pub type RBuilderEvm = revm_evm::EthCachedEvmFactory;
pub type RBuilderEvm = nethermind::NethermindEvmFactory;
