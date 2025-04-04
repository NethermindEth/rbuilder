use crate::building::evm::EvmFactory;
use alloy_primitives::{Address, Bytes};
use reth_evm::{Database, Evm, EvmEnv};
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        BlockEnv, CfgEnv, TxEnv,
    },
    inspector::NoOpInspector,
    primitives::hardfork::SpecId,
    Context, Inspector,
};

#[allow(dead_code)]
pub struct NethermindEvm<DB: Database, I> {
    inner: DB,
    inspector: I,
    inspect: bool,
}

impl<DB: Database, I: Inspector<Context<BlockEnv, TxEnv, CfgEnv, DB>>> NethermindEvm<DB, I> {
    pub fn new(inner: DB, _env: EvmEnv, inspector: I, inspect: bool) -> Self {
        Self {
            inner,
            inspector,
            inspect,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NethermindEvmFactory;

impl<DB: Database, I: Inspector<Context<BlockEnv, TxEnv, CfgEnv, DB>>> Evm
    for NethermindEvm<DB, I>
{
    type DB = DB;
    type Tx = TxEnv;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = SpecId;

    fn block(&self) -> &BlockEnv {
        unimplemented!()
    }

    fn transact_raw(&mut self, _tx: Self::Tx) -> Result<ResultAndState, Self::Error> {
        todo!()
    }

    fn transact_system_call(
        &mut self,
        _caller: Address,
        _contract: Address,
        _data: Bytes,
    ) -> Result<ResultAndState, Self::Error> {
        unimplemented!()
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        unimplemented!()
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec>) {
        unimplemented!()
    }

    fn set_inspector_enabled(&mut self, _enabled: bool) {
        unimplemented!()
    }
}

impl EvmFactory for NethermindEvmFactory {
    type EvmImpl<DB: Database, I: Inspector<Self::Context<DB>>> = NethermindEvm<DB, I>;
    type Context<DB: Database> = Context<BlockEnv, TxEnv, CfgEnv, DB>;

    fn create_evm<DB: Database>(&self, db: DB, env: EvmEnv) -> Self::EvmImpl<DB, NoOpInspector> {
        NethermindEvm::new(db, env, NoOpInspector {}, false)
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        env: EvmEnv,
        inspector: I,
    ) -> Self::EvmImpl<DB, I> {
        NethermindEvm::new(db, env, inspector, true)
    }
}
