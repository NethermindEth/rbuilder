use reth_evm::{Database, EthEvm, Evm, EvmEnv};
use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    context_interface::{
        result::{EVMError, HaltReason},
        ContextTr,
    },
    inspector::{JournalExt, NoOpInspector},
    Context, Inspector, MainBuilder, MainContext,
};

/// Main trait to create instance of EVM.
/// Allows to use different implementations of EVM with a simpler, more concrete interface than `reth_evm::EvmFactory``.
/// A type responsible for creating instances of an ethereum virtual machine given a certain input.
pub trait EvmFactory {
    type EvmImpl<DB: Database, I: Inspector<Self::Context<DB>>>: Evm<
        Tx = TxEnv,
        Error = EVMError<DB::Error>,
        HaltReason = HaltReason,
    >;
    type Context<DB: Database>: ContextTr<Journal: JournalExt>;

    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> Self::EvmImpl<DB, NoOpInspector>;

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> Self::EvmImpl<DB, I>;
}

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
