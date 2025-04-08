use crate::building::evm::{BuilderEvm, EvmFactory};
use alloy_evm::{eth::EthEvmContext, Database, EvmEnv, IntoTxEnv};
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        TxEnv,
    },
    inspector::NoOpInspector,
    Inspector,
};

#[allow(dead_code)]
pub struct NethermindEvm<DB: Database, I> {
    inner: DB,
    inspector: I,
    inspect: bool,
}

impl<DB: Database, I: Inspector<EthEvmContext<DB>>> NethermindEvm<DB, I> {
    pub fn new(inner: DB, _env: EvmEnv, inspector: I, inspect: bool) -> Self {
        Self {
            inner,
            inspector,
            inspect,
        }
    }
}

impl<DB: Database, I: Inspector<EthEvmContext<DB>>> BuilderEvm<DB> for NethermindEvm<DB, I> {
    fn transact(
        &mut self,
        _tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>> {
        todo!()
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NethermindEvmFactory;

impl EvmFactory for NethermindEvmFactory {
    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> impl BuilderEvm<DB> {
        NethermindEvm::new(db, env, NoOpInspector {}, false)
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<EthEvmContext<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> impl BuilderEvm<DB> {
        NethermindEvm::new(db, env, inspector, true)
    }
}
