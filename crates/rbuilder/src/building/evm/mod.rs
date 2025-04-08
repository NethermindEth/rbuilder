use alloy_evm::{eth::EthEvmContext, Database, Evm, EvmEnv, IntoTxEnv};
use revm::{
    context::{result::ResultAndState, TxEnv},
    context_interface::result::{EVMError, HaltReason},
    Inspector,
};

pub trait BuilderEvm<DB: Database> {
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>>;
}

impl<T, DB: Database> BuilderEvm<DB> for T
where
    T: Evm<DB = DB, Tx = TxEnv, Error = EVMError<DB::Error>, HaltReason = HaltReason>,
{
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>> {
        T::transact(self, tx)
    }
}

/// Main trait to create instance of EVM.
/// Allows to use different implementations of EVM with a simpler, more concrete interface than `reth_evm::EvmFactory`.
/// A type responsible for creating instances of an ethereum virtual machine given a certain input.
pub trait EvmFactory {
    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> impl BuilderEvm<DB>;

    fn create_evm_with_inspector<DB: Database, I: Inspector<EthEvmContext<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> impl BuilderEvm<DB>;
}

mod nethermind_evm;
mod revm_evm;

// Default to RevmEvmFactory
#[cfg(not(feature = "nethermind_evm"))]
pub type RBuilderEvm = revm_evm::RevmEvmFactory;
#[cfg(feature = "nethermind_evm")]
pub type RBuilderEvm = nethermind_evm::NethermindEvmFactory;
