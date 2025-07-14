use crate::building::evm::{Evm, EvmFactory};
use crate::building::precompile_cache::{PrecompileCache, WrappedPrecompile};
use parking_lot::Mutex;
use reth_evm::{
    eth::EthEvmContext, EthEvm, EthEvmFactory, Evm as RethEvm, EvmEnv,
    EvmFactory as RethEvmFactory, IntoTxEnv,
};
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        TxEnv,
    },
    handler::EthPrecompiles,
    interpreter::interpreter::EthInterpreter,
    primitives::hardfork::SpecId,
    Database, Inspector,
};
use std::sync::Arc;

/// Implementation of the `Evm` trait for revm (as `RethEvm`)
impl<DB, EVM> Evm<DB> for EVM
where
    DB: Database<Error: Send + Sync + 'static>,
    EVM: RethEvm<
        DB = DB,
        Tx = TxEnv,
        Error = EVMError<DB::Error>,
        HaltReason = HaltReason,
        Spec = SpecId,
    >,
{
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>> {
        EVM::transact(self, tx)
    }
}

#[derive(Debug, Clone, Default)]
pub struct EthCachedEvmFactory {
    evm_factory: EthEvmFactory,
    cache: Arc<Mutex<PrecompileCache>>,
}

/// Implementation of the `EvmFactory` trait for `EthCachedEvmFactory`.
///
/// This implementation uses `reth_evm::EthEvm` internally and provides a concrete
/// type for the `Evm` trait.
///
/// It also integrates precompile caching using the [`PrecompileCache`] and
/// [`WrappedPrecompile`] types.
impl EvmFactory for EthCachedEvmFactory {
    fn create_evm<DB>(&self, db: DB, env: EvmEnv) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
    {
        let evm = self
            .evm_factory
            .create_evm(db, env)
            .into_inner()
            .with_precompiles(WrappedPrecompile::new(
                EthPrecompiles::default(),
                self.cache.clone(),
            ));

        EthEvm::new(evm, false)
    }

    fn create_evm_with_inspector<DB, I>(&self, db: DB, env: EvmEnv, inspector: I) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<EthEvmContext<DB>, EthInterpreter>,
    {
        let evm = self
            .evm_factory
            .create_evm(db, env)
            .into_inner()
            .with_precompiles(WrappedPrecompile::new(
                EthPrecompiles::default(),
                self.cache.clone(),
            ))
            .with_inspector(inspector);

        EthEvm::new(evm, true)
    }
}
