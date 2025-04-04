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
    // The Evm receives a mutable reference to the database (not revm::DatabaseRef)
    // This is because `Evm` initially provides state mutability methods (e.g. `transact_commit`)
    // Custom Evm implementation for RBuilder only requires the implementation of the `transact_raw` method,
    // and it should not mutate the underlying database.
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

mod nethermind_evm;
mod revm_evm;

// Default to RevmEvmFactory
#[cfg(not(feature = "nethermind_evm"))]
pub type RBuilderEvm = revm_evm::RevmEvmFactory;
#[cfg(feature = "nethermind_evm")]
pub type RBuilderEvm = nethermind_evm::NethermindEvmFactory;
