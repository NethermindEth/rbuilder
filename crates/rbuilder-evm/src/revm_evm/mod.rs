use crate::{
    evm_inspector::RBuilderEVMInspector,
    tx_sim_cache::{EVMRecordingDatabase, TxStateAccessTrace},
    Evm, EvmFactory,
};
use precompile_cache::{PrecompileCache, WrappedPrecompile};

use parking_lot::Mutex;
use reth_errors::ProviderError;
use reth_evm::{
    EthEvm, EthEvmFactory, Evm as RethEvm, EvmEnv, EvmFactory as RethEvmFactory, IntoTxEnv,
};
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        TxEnv,
    },
    database::BundleState,
    handler::EthPrecompiles,
    primitives::hardfork::SpecId,
    Database,
};
use std::sync::Arc;

mod precompile_cache;

/// Implementation of the `Evm` trait for revm (as `RethEvm`)
impl<'a, EVM, DB> Evm for EVM
where
    DB: Database<Error = ProviderError>,
    EVM: RethEvm<
        DB = EVMRecordingDatabase<'a, DB>,
        Tx = TxEnv,
        Error = EVMError<DB::Error>,
        HaltReason = HaltReason,
        Spec = SpecId,
    >,
{
    fn transact(
        &mut self,
        _bundle_state: Option<BundleState>,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<ProviderError>> {
        // BundleState is not used as the db is already in sync with the current bundle state with revm
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
    fn create_evm<DB>(&self, db: DB, env: EvmEnv) -> impl Evm
    where
        DB: Database<Error = ProviderError>,
    {
        let db = EVMRecordingDatabase::new(db, false, None);
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

    fn create_evm_with_tracers<'a, DB>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: &'a mut RBuilderEVMInspector,
        recorded_state_access_trace: Option<&'a mut TxStateAccessTrace>,
    ) -> impl Evm
    where
        DB: Database<Error = ProviderError>,
    {
        let db = EVMRecordingDatabase::new(
            db,
            recorded_state_access_trace.is_some(),
            recorded_state_access_trace,
        );

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
