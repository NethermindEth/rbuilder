use std::{ops::RangeBounds, sync::Arc};

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
use dashmap::DashMap;
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
    handle: tokio::runtime::Handle,
}

impl<T> RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    pub fn new(client: RpcClient<T>) -> Self {
        let provider = ProviderBuilder::new().on_client(client);
        let handle = tokio::runtime::Handle::current();
        Self {
            handle,
            remote_provider: provider,
        }
    }
}

impl<T> StateProviderFactory for RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    /// Storage provider for latest block.
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            BlockId::Number(BlockNumberOrTag::Latest),
            self.handle.clone(),
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
            self.handle.clone(),
        ))
    }

    /// Returns a historical [StateProvider] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            BlockId::Hash(block.into()),
            self.handle.clone(),
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
    handle: tokio::runtime::Handle,
    block_id: BlockId,
    cache: DashMap<B256, Address>,
}

impl<T> RemoteStateProvider<T> {
    pub fn new(
        remote_provider: RootProvider<T>,
        block_id: BlockId,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            handle,
            remote_provider,
            block_id,
            cache: DashMap::new(),
        }
    }

    pub fn boxed(
        remote_provider: RootProvider<T>,
        block_id: BlockId,
        handle: tokio::runtime::Handle,
    ) -> Box<Self> {
        Box::new(Self::new(remote_provider, block_id, handle))
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
        let storage = tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                self.remote_provider
                    .get_storage_at(account, storage_key.into())
                    .block_id(self.block_id)
                    .await
            })
        })
        .map_err(transport_to_provider_error)?;

        Ok(Some(storage))
    }

    ///Get account code by its hash
    /// NOTE: this assumes that, usually, firstly basic_account is called (which has bytecode_hash)
    /// and only then `bytecode_hash` is called
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        let bytes = tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                if let Some(address) = self.cache.get(&code_hash) {
                    self.remote_provider
                        .get_code_at(*address)
                        .block_id(self.block_id)
                        .await
                } else {
                    //TODO: Placeholder till we have proper RPC call
                    println!("Uncached");
                    self.remote_provider
                        .get_code_at(Address::from_word(code_hash))
                        .block_id(self.block_id)
                        .await
                }
            })
        })
        .map_err(transport_to_provider_error)?;

        Ok(Some(Bytecode::new_raw(bytes)))
    }
}

// Required by the StateProvider
impl<T> BlockHashReader for RemoteStateProvider<T>
where
    T: Transport + Clone,
{
    /// Get the hash of the block with the given number. Returns `None` if no block with this number exists
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        let block = tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                self.remote_provider
                    .get_block_by_number(BlockNumberOrTag::Number(number), false.into())
                    .await
            })
        })
        .map_err(transport_to_provider_error)?
        .map(|b| b.header.hash);

        Ok(block)
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
        let account = tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                let account = self
                    .remote_provider
                    .get_proof(address, Vec::new())
                    .block_id(self.block_id)
                    .await?;

                self.cache.insert(account.code_hash, address);

                Ok(Account {
                    nonce: account.nonce,
                    bytecode_hash: account.code_hash.into(),
                    balance: account.balance,
                })
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

impl<T> StateRootCalculator for RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    fn calculate_state_root(
        &self,
        _parent_hash: B256,
        _outcome: &ExecutionOutcome,
        _sparse_trie_shared_cache: SparseTrieSharedCache,
        _config: RootHashConfig,
    ) -> Result<B256, crate::roothash::RootHashError> {
        //let block = self
        //    .client()
        //    .request::<_, Option<N::BlockResponse>>("eth_getBlockByNumber", (number, full))
        //    .await?
        //    .map(|mut block| {
        //        if !full {
        //            // this ensures an empty response for `Hashes` has the expected form
        //            // this is required because deserializing [] is ambiguous
        //            block.transactions_mut().convert_to_hashes();
        //        }
        //        block
        //    });
        let _ = self.remote_provider.client().request::<_, B256>("", ());

        Ok(B256::default())
    }
}

fn transport_to_provider_error(transport_error: TransportError) -> ProviderError {
    ProviderError::Database(reth_db::DatabaseError::Other(transport_error.to_string()))
}
