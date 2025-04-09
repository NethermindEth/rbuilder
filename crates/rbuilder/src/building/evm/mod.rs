use alloy_evm::{eth::EthEvmContext, Database, Evm, EvmEnv, IntoTxEnv};
use revm::{
    context::{result::ResultAndState, TxEnv},
    context_interface::result::{EVMError, HaltReason},
    interpreter::interpreter::EthInterpreter,
    Inspector,
};

pub trait BuilderEvm<DB: Database> {
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>>;
}

impl<DB, EVM> BuilderEvm<DB> for EVM
where
    DB: Database<Error: Send + Sync + 'static>,
    EVM: Evm<DB = DB, Tx = TxEnv, Error = EVMError<DB::Error>, HaltReason = HaltReason>,
{
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>> {
        EVM::transact(self, tx)
    }
}

/// Main trait to create instance of EVM.
/// Allows to use implementations of EVM with a simpler, more concrete interface than `reth_evm::EvmFactory`.
pub trait EvmFactory {
    fn create_evm<DB>(&self, db: DB, env: EvmEnv) -> impl BuilderEvm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>;

    fn create_evm_with_inspector<DB, I>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> impl BuilderEvm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<EthEvmContext<DB>, EthInterpreter>;
}

mod revm_evm;
pub type RBuilderEvm = revm_evm::RevmEvmFactory;
