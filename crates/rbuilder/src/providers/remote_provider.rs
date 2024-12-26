use std::ops::RangeBounds;

use crate::roothash::{RootHashConfig, StateRootCalculator};
use alloy_consensus::Header;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{
    map::{HashMap, HashSet},
    BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, U256,
};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use alloy_transport::{Transport, TransportError};
use eth_sparse_mpt::SparseTrieSharedCache;
use reth::providers::ExecutionOutcome;
use reth_chainspec::ChainInfo;
use reth_errors::{ProviderError, ProviderResult};
use reth_primitives::{Account, Bytecode, SealedHeader};
use reth_provider::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, HeaderProvider,
    StateProofProvider, StateProvider, StateProviderBox, StateProviderFactory, StateRootProvider,
    StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof, StorageProof,
    TrieInput,
};
use revm_primitives::{Address, B256};

#[derive(Clone)]
pub struct RemoteProviderFactory<T> {
    remote_provider: RootProvider<T>,
}

impl<T> RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    pub fn new(client: RpcClient<T>) -> Self {
        let provider = ProviderBuilder::new().on_client(client);
        Self {
            remote_provider: provider,
        }
    }
}

// TODO:
// Check if calls to the functions below are always used from within a tokio context
// I'm using futures::executor, instead of the tokio runtime
// This is because I'm unsure if the code here is always called from withing tokio context/runtime
// Tokio is indeed used, but some code is executed from within sys threads instead of Tokio -
// calling tokio::Handle::block_on() will result in a panic in this scenario
// Downside of the futures::executor is that it adds overhead of yet another executor which is not
// "in sync" with the tokio one

impl<T> StateProviderFactory for RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    /// Storage provider for latest block.
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            BlockId::Number(BlockNumberOrTag::Latest),
        ))
    }

    fn state_by_block_number_or_tag(
        &self,
        _number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        unimplemented!()
    }

    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox> {
        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            BlockId::Number(block.into()),
        ))
    }

    /// Returns a historical [StateProvider] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            BlockId::Hash(block.into()),
        ))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        unimplemented!()
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        unimplemented!()
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        unimplemented!()
    }
}

// Required by the StateProviderFactory
impl<T> BlockIdReader for RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    fn pending_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        unimplemented!()
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        unimplemented!()
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        unimplemented!()
    }
}

// Required by the BlockIdReader
impl<T> BlockNumReader for RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        unimplemented!()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        unimplemented!()
    }

    /// Returns the last block number associated with the last canonical header in the database.
    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        futures::executor::block_on(self.remote_provider.get_block_number())
            .map_err(transport_to_provider_error)
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        unimplemented!()
    }
}
// Required by the BlockNumReader
impl<T> BlockHashReader for RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    fn block_hash(&self, _number: BlockNumber) -> ProviderResult<Option<B256>> {
        unimplemented!()
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        unimplemented!()
    }
}

impl<T> HeaderProvider for RemoteProviderFactory<T>
where
    T: Send + Sync,
{
    /// Get header by block hash
    fn header(&self, _block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        todo!()
    }

    fn header_by_number(&self, _num: u64) -> ProviderResult<Option<Header>> {
        unimplemented!()
    }

    fn header_td(&self, _hash: &BlockHash) -> ProviderResult<Option<U256>> {
        unimplemented!()
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> ProviderResult<Option<U256>> {
        unimplemented!()
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        unimplemented!()
    }

    fn sealed_header(&self, _number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        unimplemented!()
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        unimplemented!()
    }
}

pub struct RemoteStateProvider<T> {
    remote_provider: RootProvider<T>,
    block_id: BlockId,
}

impl<T> RemoteStateProvider<T> {
    pub fn new(remote_provider: RootProvider<T>, block_id: BlockId) -> Self {
        Self {
            remote_provider,
            block_id,
        }
    }

    pub fn boxed(remote_provider: RootProvider<T>, block_id: BlockId) -> Box<Self> {
        Box::new(Self::new(remote_provider, block_id))
    }
}

impl<T> StateProvider for RemoteStateProvider<T>
where
    T: Transport + Clone,
{
    /// Get storage of given account
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let storage = futures::executor::block_on(async {
            self.remote_provider
                .get_storage_at(account, storage_key.into())
                .await
        })
        .map_err(transport_to_provider_error)?;

        Ok(Some(storage))
    }

    // TODO: C/P from Ferran's PR (https://github.com/flashbots/rbuilder/pull/144),
    // but I'm not sure this is actually correct, code_hash shouldn't be the same thing as Address

    ///Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        let bytecode = futures::executor::block_on(async {
            self.remote_provider
                .get_code_at(Address::from_word(code_hash))
                .block_id(self.block_id)
                .await
        })
        .map_err(transport_to_provider_error)?;

        Ok(Some(Bytecode::new_raw(bytecode)))
    }
}

// Required by the StateProvider
impl<T> BlockHashReader for RemoteStateProvider<T>
where
    T: Send + Sync,
{
    /// Get the hash of the block with the given number. Returns `None` if no block with this number exists
    fn block_hash(&self, _number: BlockNumber) -> ProviderResult<Option<B256>> {
        todo!()
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        unimplemented!()
    }
}

// Required by the StateProvider
impl<T> AccountReader for RemoteStateProvider<T>
where
    T: Transport + Clone,
{
    /// Get basic account information.
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        let account = futures::executor::block_on(async {
            let account = self
                .remote_provider
                .get_proof(address, Vec::new())
                .block_id(self.block_id)
                .await?;

            Ok(Account {
                nonce: account.nonce,
                bytecode_hash: account.code_hash.into(),
                balance: account.balance,
            })
        })
        .map_err(transport_to_provider_error)?;

        Ok(Some(account))
    }
}

impl<T> StateRootProvider for RemoteStateProvider<T>
where
    T: Send + Sync,
{
    fn state_root(&self, _hashed_state: HashedPostState) -> ProviderResult<B256> {
        unimplemented!()
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        unimplemented!()
    }

    fn state_root_with_updates(
        &self,
        _hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        unimplemented!()
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        unimplemented!()
    }
}

impl<T> StorageRootProvider for RemoteStateProvider<T>
where
    T: Send + Sync,
{
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        unimplemented!()
    }

    fn storage_proof(
        &self,
        _address: Address,
        _slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        unimplemented!()
    }
}

impl<T> StateProofProvider for RemoteStateProvider<T>
where
    T: Send + Sync,
{
    fn proof(
        &self,
        _input: TrieInput,
        _address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        unimplemented!()
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: HashMap<B256, HashSet<B256>>,
    ) -> ProviderResult<MultiProof> {
        unimplemented!()
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        unimplemented!()
    }
}

impl<T> StateRootCalculator for RemoteProviderFactory<T> {
    fn calculate_state_root(
        &self,
        _parent_hash: B256,
        _outcome: &ExecutionOutcome,
        _sparse_trie_shared_cache: SparseTrieSharedCache,
        _config: RootHashConfig,
    ) -> Result<B256, crate::roothash::RootHashError> {
        todo!()
    }
}

fn transport_to_provider_error(transport_error: TransportError) -> ProviderError {
    ProviderError::Database(reth_db::DatabaseError::Other(transport_error.to_string()))
}
