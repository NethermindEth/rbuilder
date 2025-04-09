use crate::building::{
    evm::{BuilderEvm, EvmFactory},
    precompile_cache::{PrecompileCache, WrappedPrecompile},
};
use alloy_evm::{eth::EthEvmContext, Database, EthEvm, EvmEnv};
use parking_lot::Mutex;
use revm::{
    handler::EthPrecompiles, inspector::NoOpInspector, interpreter::interpreter::EthInterpreter,
    Context, Inspector, MainBuilder, MainContext,
};
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
pub struct RevmEvmFactory {
    cache: Arc<Mutex<PrecompileCache>>,
}

impl EvmFactory for RevmEvmFactory {
    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> impl BuilderEvm<DB> {
        EthEvm::new(
            Context::mainnet()
                .with_block(env.block_env)
                .with_cfg(env.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(NoOpInspector {})
                .with_precompiles(WrappedPrecompile::new(
                    EthPrecompiles::default(),
                    self.cache.clone(),
                )),
            false,
        )
    }

    fn create_evm_with_inspector<DB, I>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> impl BuilderEvm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<EthEvmContext<DB>, EthInterpreter>,
    {
        EthEvm::new(
            Context::mainnet()
                .with_block(env.block_env)
                .with_cfg(env.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(inspector),
            true,
        )
    }
}
