use reth_evm::{Database, EthEvm, Evm, EvmEnv};
use revm::{
    context::TxEnv,
    context_interface::result::{EVMError, HaltReason},
    inspector::NoOpInspector,
    Context, MainBuilder, MainContext,
};

/// Main trait to create instance of EVM.
/// Allows to use different implementations of EVM with a simpler, more concrete interface than `reth_evm::EvmFactory``.
/// A type responsible for creating instances of an ethereum virtual machine given a certain input.
pub trait EvmFactory {
    type EvmImpl<DB: Database>: Evm<
        Tx = TxEnv,
        Error = EVMError<DB::Error>,
        HaltReason = HaltReason,
    >;

    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> Self::EvmImpl<DB>;

    // Creates a new instance of an EVM with an inspector.
    //
    // Note: It is expected that the [`Inspector`] is usually provided as `&mut Inspector` so that
    // it remains owned by the call site when [`Evm::transact`] is invoked.

    // fn create_evm_with_inspector<DB: Database>(
    //     &self,
    //     db: DB,
    //     env: EvmEnv,
    //     inspector: RBuilderEVMInspector,
    // ) -> Self::EvmImpl<DB>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RevmEvmFactory;

impl EvmFactory for RevmEvmFactory {
    type EvmImpl<DB: Database> = EthEvm<DB, NoOpInspector>;

    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> Self::EvmImpl<DB> {
        EthEvm::new(
            Context::mainnet()
                .with_block(env.block_env)
                .with_cfg(env.cfg_env)
                .with_db(db)
                .build_mainnet_with_inspector(NoOpInspector {}),
            false,
        )
    }

    // fn create_evm_with_inspector ...
}
