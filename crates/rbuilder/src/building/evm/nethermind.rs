use crate::building::evm::{Evm, EvmFactory};
use reth_evm::{eth::EthEvmContext, EvmEnv, IntoTxEnv};
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        TxEnv,
    },
    interpreter::interpreter::EthInterpreter,
    Database, Inspector,
};

struct NethermindEvm<DB: Database<Error: Send + Sync + 'static>> {
    db: DB,
}

impl<DB: Database<Error: Send + Sync + 'static>> Evm<DB> for NethermindEvm<DB> {
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>> {
        todo!()
    }
}

#[derive(Debug, Clone, Default)]
pub struct NethermindEvmFactory {}

/// Implementation of the `EvmFactory` trait for `NethermindEvmFactory`.
impl EvmFactory for NethermindEvmFactory {
    fn create_evm<DB>(&self, db: DB, env: EvmEnv) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
    {
        NethermindEvm { db }
    }

    fn create_evm_with_inspector<DB, I>(&self, db: DB, env: EvmEnv, inspector: I) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<EthEvmContext<DB>, EthInterpreter>,
    {
        NethermindEvm { db }
    }
}
