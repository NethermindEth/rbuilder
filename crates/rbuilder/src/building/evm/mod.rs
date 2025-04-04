use reth_evm::{Database, Evm, EvmEnv};
use revm::{
    context::TxEnv,
    context_interface::{
        result::{EVMError, HaltReason},
        ContextTr,
    },
    inspector::{JournalExt, NoOpInspector},
    Inspector,
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

mod revm_evm;
pub use revm_evm::RevmEvmFactory;

mod nethermind_evm;
pub use nethermind_evm::NethermindEvmFactory;
