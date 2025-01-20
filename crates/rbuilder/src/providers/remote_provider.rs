use std::{collections::HashMap, ops::RangeBounds, sync::Arc};

use crate::roothash::{RootHashConfig, StateRootCalculator};
use alloy_consensus::Header;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{
    map::HashSet, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, U256,
};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use alloy_transport::{Transport, TransportError};
use dashmap::DashMap;
use eth_sparse_mpt::SparseTrieSharedCache;
use foldhash::HashMapExt;
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
use revm::db::{AccountStatus, BundleAccount, BundleState};
use revm_primitives::{Address, B256};
use serde::{Deserialize, Serialize};

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
    T: Transport + Clone,
{
    /// Get header by block hash
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        let block = tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                self.remote_provider
                    .get_block_by_hash(*block_hash, false.into())
                    .await
            })
        })
        .map_err(transport_to_provider_error)?;

        Ok(block.map(|b| {
            println!("block: {}", b.header.number);
            b.header.inner
        }))
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Header>> {
        let block = tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                self.remote_provider
                    .get_block_by_number(num.into(), false.into())
                    .await
            })
        })
        .map_err(transport_to_provider_error)?;

        Ok(block.map(|b| b.header.inner))
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
        let bytes = match tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                self.remote_provider
                    .client()
                    .request::<_, Bytes>("rbuilder_getCodeByHash", (code_hash,))
                    .await
                //if let Some(address) = self.cache.get(&code_hash) {
                //    self.remote_provider
                //        .get_code_at(*address)
                //        .block_id(self.block_id)
                //        .await
                //} else {
                //    //TODO: Placeholder till we have proper RPC call
                //    println!("Uncached");
                //    self.remote_provider
                //        .get_code_at(Address::from_word(code_hash))
                //        .block_id(self.block_id)
                //        .await
                //}
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                println!("Err: {e}");
                return Err(transport_to_provider_error(e));
            }
        };

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
        _targets: std::collections::HashMap<
            alloy_primitives::FixedBytes<32>,
            std::collections::HashSet<
                alloy_primitives::FixedBytes<32>,
                foldhash::fast::RandomState,
            >,
            foldhash::fast::RandomState,
        >,
    ) -> ProviderResult<MultiProof> {
        unimplemented!()
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
    ) -> std::result::Result<
        std::collections::HashMap<
            alloy_primitives::FixedBytes<32>,
            alloy_primitives::Bytes,
            foldhash::fast::RandomState,
        >,
        reth_errors::ProviderError,
    > {
        unimplemented!()
    }
}

impl<T> StateRootCalculator for RemoteProviderFactory<T>
where
    T: Transport + Clone,
{
    fn calculate_state_root(
        &self,
        parent_hash: B256,
        outcome: &ExecutionOutcome,
        _sparse_trie_shared_cache: SparseTrieSharedCache,
        _config: RootHashConfig,
    ) -> Result<B256, crate::roothash::RootHashError> {
        let hashed_state = outcome.hash_state_slow();
        let accounts_diff: HashMap<Address, AccountDiff> =
            HashMap::with_capacity(hashed_state.accounts.len());

        let account_diff: HashMap<Address, AccountDiff> = outcome
            .bundle
            .state
            .iter()
            .map(|(address, diff)| (*address, diff.clone().into()))
            .collect();

        let hash = match tokio::task::block_in_place(move || {
            self.handle.block_on(async move {
                self.remote_provider
                    .client()
                    .request::<_, B256>(
                        "rbuilder_calculateStateRoot",
                        (BlockId::Hash(parent_hash.into()), account_diff),
                    )
                    .await
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                println!("Err: {e}");
                return Err(crate::roothash::RootHashError::Verification);
            }
        };

        Ok(hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AccountDiff {
    pub nonce: Option<U256>,
    pub balance: Option<U256>,
    pub code: Option<Bytes>,
    pub self_destructed: bool,
    pub changed_slots: HashMap<U256, U256>,
    #[serde(skip)]
    pub code_hash: Option<B256>,
    #[serde(skip)]
    pub changed: bool,
}

impl From<BundleAccount> for AccountDiff {
    fn from(value: BundleAccount) -> Self {
        let self_destructed = value.was_destroyed();
        if self_destructed {
            println!("!!!!! SELF DESTRUCTED !!!!!");
        }

        let changed_slots = if self_destructed {
            HashMap::new()
        } else {
            value
                .storage
                .iter()
                .map(|(k, v)| {
                    println!("Storage: K: {:?}, V: {:?}", k, v.present_value);
                    (*k, v.present_value)
                })
                .collect()
        };

        match value.info {
            Some(info) => {
                let code = info.code.map(|c| c.bytes());
                //println!("Balance {}", info.balance);
                //println!("Nonce {}", info.nonce);
                //println!(
                //    "Code hash {}",
                //    if code.is_some() {
                //        "has code"
                //    } else {
                //        "no code"
                //    }
                //);
                //println!("Code hash: {}", info.code_hash);
                //
                Self {
                    changed_slots,
                    self_destructed,
                    balance: if info.balance == U256::ZERO {
                        None
                    } else {
                        Some(info.balance)
                    },
                    nonce: Some(U256::from(info.nonce)),
                    code_hash: Some(info.code_hash),
                    code,
                    //TODO: implement this if it will bring perf improvements there is status flag and check for
                    //value.is_info_changed
                    changed: false,
                    delete: false,
                }
            }
            None => {
                println!("Account is none");

                Self {
                    changed_slots,
                    self_destructed,
                    balance: None,
                    nonce: None,
                    code_hash: None,
                    code: None,
                    changed: false,
                    delete: true,
                }
            }
        }
    }
}

fn transport_to_provider_error(transport_error: TransportError) -> ProviderError {
    ProviderError::Database(reth_db::DatabaseError::Other(transport_error.to_string()))
}
