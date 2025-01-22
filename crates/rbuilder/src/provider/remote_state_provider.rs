use std::future::Future;

use alloy_consensus::Header;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{BlockHash, BlockNumber};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use alloy_transport::{Transport, TransportError};
use reth_errors::{ProviderError, ProviderResult};
use reth_provider::StateProviderBox;
use revm_primitives::B256;

use super::StateProviderFactory;

/// Remote state provider factory allows providing state via remote RPC calls
/// using either IPC or HTTP/WS
pub struct RemoteStateProviderFactory<T> {
    remote_provider: RootProvider<T>,
}

impl<T> RemoteStateProviderFactory<T>
where
    T: Transport + Clone,
{
    pub fn new(client: RpcClient<T>) -> Self {
        let remote_provider = ProviderBuilder::new().on_client(client);

        Self { remote_provider }
    }

    // StateProviderFactory requires sync context, but calls to remote provider are async
    // What's more, rbuilder is executed in async context, so we have situation
    // async -> sync -> async
    fn run_future<F, R>(&self, f: F) -> R
    where
        F: Future<Output = R>,
    {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
    }
}

impl<T> StateProviderFactory for RemoteStateProviderFactory<T>
where
    T: Transport + Clone,
{
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        todo!()
    }

    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        let future = self
            .remote_provider
            .get_block_by_number(BlockNumberOrTag::Number(number), false.into());

        let block_hash = self
            .run_future(future)
            .map_err(transport_to_provider_error)?
            .map(|b| b.header.hash);

        Ok(block_hash)
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        todo!()
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Header>> {
        todo!()
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        todo!()
    }

    fn root_hasher(&self, parent_hash: B256) -> Box<dyn super::RootHasher> {
        todo!()
    }
}

fn transport_to_provider_error(transport_error: TransportError) -> ProviderError {
    ProviderError::Database(reth_db::DatabaseError::Other(transport_error.to_string()))
}
