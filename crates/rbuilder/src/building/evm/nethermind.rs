use crate::building::evm::{Evm, EvmFactory};
use crate::provider::ipc_provider::DEFAULT_IPC_REQUEST_TIMEOUT_MS;
use reipc::rpc_provider::RpcProvider;
use reth_evm::{eth::EthEvmContext, EvmEnv, IntoTxEnv};
use revm::{
    context::{
        result::{EVMError, HaltReason, ResultAndState},
        TxEnv,
    },
    interpreter::interpreter::EthInterpreter,
    Database, Inspector,
};
use std::{path::Path, time::Duration};

#[derive(Debug, Clone)]
struct NethermindRpcProvider(RpcProvider);

// TODO: use IpcProviderConfig/BaseConfig instead of hardcoded path
impl Default for NethermindRpcProvider {
    fn default() -> Self {
        NethermindRpcProvider(
            RpcProvider::try_connect(
                Path::new("/tmp/nethermind.ipc"),
                Some(Duration::from_millis(DEFAULT_IPC_REQUEST_TIMEOUT_MS)),
            )
            .expect("can't connect to IPC (evm factory)"),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct NethermindEvmFactory {
    ipc_provider: NethermindRpcProvider,
}

/// Implementation of the `EvmFactory` trait for `NethermindEvmFactory`.
impl EvmFactory for NethermindEvmFactory {
    fn create_evm<DB>(&self, db: DB, env: EvmEnv) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
    {
        NethermindEvm::new(self.ipc_provider.clone(), db, env)
    }

    // TODO: Link inspector to evm instance
    fn create_evm_with_inspector<DB, I>(&self, db: DB, env: EvmEnv, inspector: I) -> impl Evm<DB>
    where
        DB: Database<Error: Send + Sync + 'static>,
        I: Inspector<EthEvmContext<DB>, EthInterpreter>,
    {
        NethermindEvm::new(self.ipc_provider.clone(), db, env)
    }
}

struct NethermindEvm<DB: Database<Error: Send + Sync + 'static>> {
    ipc_provider: NethermindRpcProvider,
    db: DB,
    env: EvmEnv,
}

impl<DB: Database<Error: Send + Sync + 'static>> NethermindEvm<DB> {
    fn new(ipc_provider: NethermindRpcProvider, db: DB, env: EvmEnv) -> Self {
        Self {
            ipc_provider,
            db,
            env,
        }
    }
}

impl<DB: Database<Error: Send + Sync + 'static>> Evm<DB> for NethermindEvm<DB> {
    fn transact(
        &mut self,
        tx: impl IntoTxEnv<TxEnv>,
    ) -> Result<ResultAndState<HaltReason>, EVMError<DB::Error>> {
        // TODO
        // 1. get state diff from self.db
        // 2. send to NMC: self.ipc_provider.0.call("rbuilder_transact", {
        //    "tx": tx,
        //    "state_diff": state_diff,
        //    "env": self.env,
        // })
        // 3. get result from NMC + used state + access list
        // 4. populate correctly self.inspector
        // 5. return result

        let result = self
            .ipc_provider
            .0
            .call::<TxEnv, ResultAndState<HaltReason>>("rbuilder_transact", tx.into_tx_env())
            .map_err(|e| ipc_to_evm_error::<DB>(e))?;

        Ok(result)
    }
}

// TODO: error mapping
fn ipc_to_evm_error<DB: Database<Error: Send + Sync + 'static>>(
    e: reipc::errors::RpcError,
) -> EVMError<DB::Error> {
    EVMError::Custom(e.to_string())
}
