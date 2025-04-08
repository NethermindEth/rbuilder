use crate::building::evm::{BuilderEvm, EvmFactory};
use alloy_evm::{eth::EthEvmContext, Database, EthEvm, EvmEnv};
use revm::{inspector::NoOpInspector, Context, Inspector, MainBuilder, MainContext};

#[derive(Debug, Default, Clone, Copy)]
pub struct RevmEvmFactory;

impl EvmFactory for RevmEvmFactory {
    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> impl BuilderEvm<DB> {
        EthEvm::new(
            Context::mainnet()
                .with_block(env.block_env)
                .with_cfg(env.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(NoOpInspector {}),
            false,
        )
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<EthEvmContext<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> impl BuilderEvm<DB> {
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
