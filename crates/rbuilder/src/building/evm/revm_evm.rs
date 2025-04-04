use crate::building::evm::EvmFactory;
use reth_evm::{Database, EthEvm, EvmEnv};
use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    inspector::NoOpInspector,
    Context, Inspector, MainBuilder, MainContext,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct RevmEvmFactory;

impl EvmFactory for RevmEvmFactory {
    type EvmImpl<DB: Database, I: Inspector<Self::Context<DB>>> = EthEvm<DB, I>;
    type Context<DB: Database> = Context<BlockEnv, TxEnv, CfgEnv, DB>;

    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> Self::EvmImpl<DB, NoOpInspector> {
        EthEvm::new(
            Context::mainnet()
                .with_block(env.block_env)
                .with_cfg(env.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(NoOpInspector {}),
            false,
        )
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> Self::EvmImpl<DB, I> {
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
