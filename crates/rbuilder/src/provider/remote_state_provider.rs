use std::{error::Error, fmt::Debug, path::Path, sync::Arc, time::Duration};

use alloy_consensus::{constants::KECCAK_EMPTY, Header};
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{BlockHash, BlockNumber, StorageKey, StorageValue, U64};
use alloy_provider::{Provider, RootProvider};
use alloy_transport::{Transport, TransportError};
use dashmap::DashMap;
use reipc::rpc_provider::RpcProvider;
use reth_errors::{ProviderError, ProviderResult};
use reth_primitives::{Account, Bytecode};
use reth_provider::{
    AccountReader, BlockHashReader, HashedPostStateProvider, StateProofProvider, StateProvider,
    StateProviderBox, StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof,
    MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use revm::db::{BundleAccount, BundleState};
use revm_primitives::{map::B256HashMap, Address, Bytes, HashMap, B256, U256};
use serde::{de, Deserialize, Serialize};
use tokio::{runtime::Builder, sync::broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug_span, info, trace, trace_span};

use crate::live_builder::simulation::SimulatedOrderCommand;

use super::{RootHasher, StateProviderFactory};

/// Remote state provider factory allows providing state via remote RPC calls
/// using either IPC or HTTP/WS
#[derive(Clone)]
pub struct RemoteStateProviderFactory {
    remote_provider: RpcProvider,

    //block_hash_cache: Arc<DashMap<u64, BlockHash>>,
    code_cache: Arc<DashMap<B256, Bytecode>>,

    //state_provider_by_num: Arc<DashMap<BlockNumber, Box<ArcRemoteStateProvider<T>>>>,
    state_provider_by_hash: Arc<DashMap<BlockHash, Box<ArcRemoteStateProvider>>>,
}

impl RemoteStateProviderFactory {
    pub fn new(path: &Path) -> Self {
        let remote_provider = RpcProvider::try_connect(path, Duration::from_millis(10).into())
            .expect("Can't connect to IPC");

        //let future_runner = FutureRunner::new();

        Self {
            remote_provider,
            code_cache: Arc::new(DashMap::new()),
            // state_provider_by_num: Arc::new(DashMap::new()),
            state_provider_by_hash: Arc::new(DashMap::new()),
            //account_cache: Arc:BlockNumber:new(DashMap::new()),
            //block_num_cache: Arc::new(DashMap::new()),
        }
    }
}

impl StateProviderFactory for RemoteStateProviderFactory {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        //info!("Latest state provider");
        //let num = self.best_block_number()?;
        //if let Some(state) = self.state_provider_by_num.get(&num) {
        //    return Ok(state.clone());
        //}

        let state = RemoteStateProvider::new(
            self.remote_provider.clone(),
            BlockNumberOrTag::Latest.into(),
            //self.block_hash_cache.clone(),
            // self.block_num_cache.clone(),
            self.code_cache.clone(),
            //self.account_cache.clone(),
        );

        let state = ArcRemoteStateProvider::boxed(state);
        //        self.state_provider_by_num.insert(num, state.clone());
        Ok(state)
    }

    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox> {
        //if let Some(state) = self.state_provider_by_num.get(&block) {
        //    return Ok(state.clone());
        //}

        let state = RemoteStateProvider::new(
            self.remote_provider.clone(),
            //self.future_runner.clone(),
            block.into(),
            //self.block_hash_cache.clone(),
            //self.block_num_cache.clone(),
            self.code_cache.clone(),
            //self.account_cache.clone(),
        );

        let state = ArcRemoteStateProvider::boxed(state);
        //self.state_provider_by_num.insert(block, state.clone());
        Ok(state)
    }

    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        if let Some(state) = self.state_provider_by_hash.get(&block) {
            return Ok(state.clone());
        }

        let state = RemoteStateProvider::new(
            self.remote_provider.clone(),
            //self.future_runner.clone(),
            block.into(),
            //self.block_hash_cache.clone(),
            //self.block_num_cache.clone(),
            self.code_cache.clone(),
            //self.account_cache.clone(),
        );

        let state = ArcRemoteStateProvider::boxed(state);
        self.state_provider_by_hash.insert(block, state.clone());
        Ok(state)
    }

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        let id: u64 = rand::random();
        let span = trace_span!("header", id, block_hash = %block_hash.to_string());
        let _guard = span.enter();
        trace!("header: get");

        let header = self
            .remote_provider
            .call::<_, Option<<alloy_network::Ethereum as alloy_network::Network>::BlockResponse>>(
                "eth_getBlockByHash",
                (block_hash, false),
            )
            .map_err(|e| transport_to_provider_error(e.into()))?
            .map(|b| b.header.inner);

        if header.is_none() {
            trace!("header: got none");
            return Ok(None);
        }

        let header = header.unwrap();

        //self.block_hash_cache.insert(header.number, *block_hash);
        trace!("header: got");

        Ok(Some(header))
    }

    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        let id: u64 = rand::random();
        let span = trace_span!("block_hash 1", id, block_num = number);
        let _guard = span.enter();
        trace!("block_hash:get");

        //if let Some(hash) = self.block_hash_cache.get(&number) {
        //    trace!("block_hash:cache hit");
        //    return Ok(Some(*hash));
        //}

        let block_hash = self
            .remote_provider
            .call::<_, B256>("rbuilder_getBlockHash", (BlockNumberOrTag::Number(number),))
            .map_err(|e| transport_to_provider_error(e.into()))?;

        // self.block_hash_cache.insert(number, block_hash);
        trace!("block_hash: got");
        Ok(Some(block_hash))
    }

    //TODO: is this correct?
    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        //println!("best block num");
        self.last_block_number()
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Header>> {
        let id: u64 = rand::random();
        let span = trace_span!("header_by_number", id, block_num = num);
        let _guard = span.enter();
        trace!("header_by_num:get");

        let block = self
            .remote_provider
            .call::<_, Option<<alloy_network::Ethereum as alloy_network::Network>::BlockResponse>>(
                "eth_getBlockByNumber",
                (num, false),
            )
            .map_err(|e| transport_to_provider_error(e.into()))?;

        if block.is_none() {
            trace!("header_by_num: got none");
            return Ok(None);
        }

        let block = block.unwrap();
        let header = block.header.inner;

        //self.block_hash_cache.insert(header.number, hash);

        trace!("header_by_num: got");
        Ok(Some(header))
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        let id: u64 = rand::random();
        let span = trace_span!("last_block_num", id);
        let _guard = span.enter();
        trace!("last_block_num:get");

        let block_num = self
            .remote_provider
            .call::<_, U64>("eth_blockNumber", ())
            .map_err(|e| transport_to_provider_error(e.into()))?
            .to::<u64>();

        trace!("last_block_num: got");
        Ok(block_num)
    }

    fn root_hasher(&self, parent_hash: B256) -> ProviderResult<Box<dyn RootHasher>> {
        Ok(Box::new(StatRootHashCalculator {
            remote_provider: self.remote_provider.clone(),
            parent_hash,
        }))
    }
}

#[derive(Clone)]
pub struct RemoteStateProvider {
    remote_provider: RpcProvider,

    storage_cache: DashMap<(Address, StorageKey), StorageValue>,
    account_cache: DashMap<Address, Account>,

    block_id: BlockId,

    block_hash_cache: Arc<DashMap<u64, BlockHash>>,
    code_cache: Arc<DashMap<B256, Bytecode>>,
}

#[derive(Clone)]
pub struct ArcRemoteStateProvider {
    inner: Arc<RemoteStateProvider>,
}

impl ArcRemoteStateProvider {
    pub fn new(inner: RemoteStateProvider) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn boxed(inner: RemoteStateProvider) -> Box<Self> {
        Box::new(Self {
            inner: Arc::new(inner),
        })
    }
}

impl RemoteStateProvider {
    /// Crates new instance of state provider
    fn new(
        remote_provider: RpcProvider,
        block_id: BlockId,
        code_cache: Arc<DashMap<B256, Bytecode>>,
    ) -> Self {
        Self {
            remote_provider,
            block_id,
            block_hash_cache: Arc::new(DashMap::new()),

            code_cache,

            storage_cache: DashMap::new(),
            account_cache: DashMap::new(),
        }
    }

    /// Crates new instance of state provider on the heap
    fn boxed(
        remote_provider: RpcProvider,
        block_id: BlockId,
        block_hash_cache: Arc<DashMap<u64, BlockHash>>,
        code_cache: Arc<DashMap<B256, Bytecode>>,
    ) -> Box<Self> {
        Box::new(Self::new(remote_provider, block_id, code_cache))
    }
}

impl StateProvider for ArcRemoteStateProvider {
    /// Get storage of given account
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let id: u64 = rand::random();
        let span = trace_span!("storage", id);
        let _guard = span.enter();
        trace!("storage:get");

        if let Some(storage) = self.inner.storage_cache.get(&(account, storage_key)) {
            let storage_val = *storage;
            trace!("got storage from cache");
            return Ok(Some(storage_val));
        }

        let key: U256 = storage_key.into();
        let storage = self
            .inner
            .remote_provider
            .call::<_, StorageValue>("eth_getStorageAt", (account, key))
            .map_err(|e| transport_to_provider_error(e.into()))?;

        self.inner
            .storage_cache
            .insert((account, storage_key), storage);

        trace!("got storage");
        Ok(Some(storage))
    }

    /// Get account code by its hash
    /// IMPORTANT: Assumes remote provider (node) has RPC call:"rbuilder_getCodeByHash"
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        let id: u64 = rand::random();
        let span = trace_span!("bytecode", id);
        let _guard = span.enter();
        trace!("bytecode:get");

        if code_hash.is_zero() {
            trace!("bytecode: hash is zero");
            return Ok(None);
        }

        if *code_hash == KECCAK_EMPTY {
            trace!("bytecode: hash is empty");
            return Ok(None);
        }

        if let Some(bytecode) = self.inner.code_cache.get(code_hash) {
            trace!("bytecode: cache hit");
            return Ok(Some(bytecode.clone()));
        }

        let bytes = self
            .inner
            .remote_provider
            .call::<_, Bytes>("rbuilder_getCodeByHash", (code_hash,))
            .map_err(|e| transport_to_provider_error(e.into()))?;

        let bytecode = Bytecode::new_raw(bytes);

        self.inner.code_cache.insert(*code_hash, bytecode.clone());
        trace!("bytecode: got");

        Ok(Some(bytecode))
    }
}

impl BlockHashReader for ArcRemoteStateProvider {
    /// Get the hash of the block with the given number. Returns `None` if no block with this number exists
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        //let id: u64 = rand::random();
        //let span = trace_span!("block_hash 2:", id);
        //let _guard = span.enter();
        //trace!("block_hash 2: get");
        //
        if let Some(hash) = self.inner.block_hash_cache.get(&number) {
            // trace!("block_hash 2: cache hit");
            return Ok(Some(*hash));
        }
        let block_hash = self
            .inner
            .remote_provider
            .call::<_, B256>("rbuilder_getBlockHash", (BlockNumberOrTag::Number(number),))
            .map_err(|e| transport_to_provider_error(e.into()))?;

        self.inner.block_hash_cache.insert(number, block_hash);
        //trace!("block_hash 2: got");
        Ok(Some(block_hash))
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        unimplemented!()
    }
}

impl AccountReader for ArcRemoteStateProvider {
    /// Get basic account information.
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        let id: u64 = rand::random();
        let span = trace_span!("account", id, address = address.to_string());
        let _guard = span.enter();
        trace!("account: get");

        if let Some(account) = self.inner.account_cache.get(address) {
            trace!("account cache hit");
            return Ok(Some(*account));
        }

        let account = self
            .inner
            .remote_provider
            .call::<_, AccountState>("rbuilder_getAccount", (*address, self.inner.block_id))
            .map_err(|e| transport_to_provider_error(e.into()))?;

        let account = Account {
            nonce: account.nonce.try_into().unwrap(),
            bytecode_hash: account.code_hash.into(),
            balance: account.balance,
        };

        self.inner.account_cache.insert(*address, account);

        trace!("account: got");
        Ok(Some(account))
    }
}

impl StateRootProvider for ArcRemoteStateProvider {
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

impl StorageRootProvider for ArcRemoteStateProvider {
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

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        unimplemented!()
    }
}

impl StateProofProvider for ArcRemoteStateProvider {
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
        _targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        unimplemented!()
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
    ) -> ProviderResult<B256HashMap<Bytes>> {
        unimplemented!()
    }
}

impl HashedPostStateProvider for ArcRemoteStateProvider {
    fn hashed_post_state(&self, _bundle_state: &BundleState) -> HashedPostState {
        unimplemented!("hashed_post_state not used in remote provider")
    }
}

#[derive(Clone)]
pub struct StatRootHashCalculator {
    remote_provider: RpcProvider,
    parent_hash: B256,
}

impl RootHasher for StatRootHashCalculator {
    fn run_prefetcher(
        &self,
        _simulated_orders: broadcast::Receiver<SimulatedOrderCommand>,
        _cancel: CancellationToken,
    ) {
        unimplemented!()
    }

    fn state_root(
        &self,
        outcome: &reth_provider::ExecutionOutcome,
    ) -> Result<B256, crate::roothash::RootHashError> {
        let id: u64 = rand::random();
        let span = trace_span!("state_root", id, block = outcome.first_block);
        let _guard = span.enter();
        trace!("state_root: get");

        let account_diff: HashMap<Address, AccountDiff> = outcome
            .bundle
            .state
            .iter()
            .map(|(address, diff)| (*address, diff.clone().into()))
            .collect();

        let hash = self
            .remote_provider
            .call::<_, B256>(
                "rbuilder_calculateStateRoot",
                (BlockId::Hash(self.parent_hash.into()), account_diff),
            )
            .map_err(|_| crate::roothash::RootHashError::Verification)?;

        trace!("state_root: got");

        Ok(hash)
    }
}

impl derive_more::Debug for StatRootHashCalculator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatRootHashCalculator").finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AccountDiff {
    pub nonce: Option<U256>,
    pub balance: Option<U256>,
    pub self_destructed: bool,
    pub changed_slots: HashMap<U256, U256>,
    pub code_hash: Option<B256>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AccountState {
    pub nonce: U256,
    pub balance: U256,
    pub code_hash: B256,
}

impl From<BundleAccount> for AccountDiff {
    fn from(value: BundleAccount) -> Self {
        let self_destructed = value.was_destroyed();

        let changed_slots = value
            .storage
            .iter()
            .map(|(k, v)| (*k, v.present_value))
            .collect();

        match value.info {
            Some(info) => Self {
                changed_slots,
                self_destructed,
                balance: Some(info.balance),
                nonce: Some(U256::from(info.nonce)),
                code_hash: Some(info.code_hash),
            },
            None => Self {
                changed_slots,
                self_destructed,
                balance: None,
                nonce: None,
                code_hash: None,
            },
        }
    }
}

//TODO: this is temp hack, fix it properly
fn transport_to_provider_error(e: reipc::errors::RpcError) -> ProviderError {
    ProviderError::Database(reth_db::DatabaseError::Other(format!("IPC ERROR: {e}")))
}
