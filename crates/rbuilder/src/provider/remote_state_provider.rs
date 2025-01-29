use std::{
    fmt::Debug,
    future::{Future, IntoFuture},
    sync::Arc,
};

use alloy_consensus::{constants::KECCAK_EMPTY, Header};
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::{BlockHash, BlockNumber, StorageKey, StorageValue};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::RpcClient;
use alloy_transport::{Transport, TransportError};
use dashmap::DashMap;
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
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::live_builder::simulation::SimulatedOrderCommand;

use super::{RootHasher, StateProviderFactory};

/// Remote state provider factory allows providing state via remote RPC calls
/// using either IPC or HTTP/WS
#[derive(Clone)]
pub struct RemoteStateProviderFactory<T> {
    remote_provider: RootProvider<T>,
    future_runner: FutureRunner,
    block_hash_cache: Arc<DashMap<u64, BlockHash>>,
}

impl<T> RemoteStateProviderFactory<T>
where
    T: Transport + Clone,
{
    pub fn new(client: RpcClient<T>) -> Self {
        let remote_provider = ProviderBuilder::new().on_client(client);
        let future_runner = FutureRunner::new();

        Self {
            remote_provider,
            future_runner,
            block_hash_cache: Arc::new(DashMap::new()),
        }
    }

    pub fn from_provider(root_provider: RootProvider<T>) -> Self {
        let future_runner = FutureRunner::new();

        Self {
            remote_provider: root_provider,
            future_runner,
            block_hash_cache: Arc::new(DashMap::new()),
        }
    }
}

impl<T> StateProviderFactory for RemoteStateProviderFactory<T>
where
    T: Transport + Clone + Debug,
{
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        //println!("latest");
        let num = self.best_block_number()?;

        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            self.future_runner.clone(),
            BlockId::Number(num.into()),
            self.block_hash_cache.clone(),
        ))
    }

    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox> {
        //println!("history by block num {block}");
        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            self.future_runner.clone(),
            BlockId::Number(block.into()),
            self.block_hash_cache.clone(),
        ))
    }

    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox> {
        //println!("history by block hash {block}");

        //let future = self.remote_provider.get_block_by_hash(block, false.into());
        //
        //let _block_hash = match self.future_runner.run(future) {
        //    Ok(block) => block,
        //    Err(e) => {
        //        println!("error {e}");
        //        return Err(transport_to_provider_error(e));
        //    }
        //};

        Ok(RemoteStateProvider::boxed(
            self.remote_provider.clone(),
            self.future_runner.clone(),
            BlockId::Hash(block.into()),
            self.block_hash_cache.clone(),
        ))
    }

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        //println!("Get header");
        //       return Ok(Some(Header::default()));

        let future = self
            .remote_provider
            .get_block_by_hash(*block_hash, false.into());

        let header = self
            .future_runner
            .run(future)
            .map_err(transport_to_provider_error)?
            .map(|b| b.header.inner);

        if header.is_none() {
            debug!("header by hash cache miss, got none {block_hash}");
            return Ok(None);
        }

        debug!("header by hash cache miss, got header {block_hash}");
        let header = header.unwrap();

        self.block_hash_cache.insert(header.number, *block_hash);

        Ok(Some(header))
    }

    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        if let Some(hash) = self.block_hash_cache.get(&number) {
            return Ok(Some(*hash));
        }
        let future = self
            .remote_provider
            .client()
            .request::<_, B256>("rbuilder_getBlockHash", (BlockNumberOrTag::Number(number),));
        let block_hash = self
            .future_runner
            .run(future)
            .map_err(transport_to_provider_error)?;

        self.block_hash_cache.insert(number, block_hash);
        Ok(Some(block_hash))
    }

    //TODO: is this correct?
    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        //println!("best block num");
        self.last_block_number()
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Header>> {
        //return Ok(Some(Header::default()));
        //return Ok(None);

        let future = self
            .remote_provider
            .get_block_by_number(num.into(), false.into());

        let block = self
            .future_runner
            .run(future)
            .map_err(transport_to_provider_error)?;
        //.map(|b| b.header.inner);

        if block.is_none() {
            debug!("header by hash cache miss, got none {num}");
            return Ok(None);
        }

        let block = block.unwrap();
        let hash = block.header.hash;
        let header = block.header.inner;

        self.block_hash_cache.insert(header.number, hash);

        Ok(Some(header))
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        //println!("header by number");
        //return Ok(0);
        let future = self.remote_provider.get_block_number();

        let block_num = self
            .future_runner
            .run(future)
            .map_err(transport_to_provider_error)?;

        Ok(block_num)
    }

    fn root_hasher(&self, parent_hash: B256) -> ProviderResult<Box<dyn RootHasher>> {
        Ok(Box::new(StatRootHashCalculator {
            remote_provider: self.remote_provider.clone(),
            future_runner: self.future_runner.clone(),
            parent_hash,
        }))
    }
}

pub struct RemoteStateProvider<T> {
    remote_provider: RootProvider<T>,
    future_runner: FutureRunner,
    block_hash_cache: Arc<DashMap<u64, BlockHash>>,
    block_id: BlockId,
    account_cache: DashMap<Address, Account>,
    bytecode_cache: DashMap<B256, Bytecode>,
    storage_cache: DashMap<(Address, StorageKey), StorageValue>,
}

impl<T> RemoteStateProvider<T> {
    /// Crates new instance of state provider
    fn new(
        remote_provider: RootProvider<T>,
        future_runner: FutureRunner,
        block_id: BlockId,
        block_hash_cache: Arc<DashMap<u64, BlockHash>>,
    ) -> Self {
        Self {
            remote_provider,
            block_id,
            block_hash_cache,
            future_runner,
            account_cache: DashMap::new(),
            bytecode_cache: DashMap::new(),
            storage_cache: DashMap::new(),
        }
    }

    /// Crates new instance of state provider on the heap
    fn boxed(
        remote_provider: RootProvider<T>,
        future_runner: FutureRunner,
        block_id: BlockId,
        block_hash_cache: Arc<DashMap<u64, BlockHash>>,
    ) -> Box<Self> {
        Box::new(Self::new(
            remote_provider,
            future_runner,
            block_id,
            block_hash_cache,
        ))
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
        //return Ok(None);
        if let Some(storage) = self.storage_cache.get(&(account, storage_key)) {
            let storage_val = *storage;
            return Ok(Some(storage_val));
        }

        let future = self
            .remote_provider
            .get_storage_at(account, storage_key.into())
            .block_id(self.block_id)
            .into_future();

        let storage = self
            .future_runner
            .run(future)
            .map_err(transport_to_provider_error)?;

        self.storage_cache.insert((account, storage_key), storage);

        Ok(Some(storage))
    }

    /// Get account code by its hash
    /// IMPORTANT: Assumes remote provider (node) has RPC call:"rbuilder_getCodeByHash"
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        return Ok(None);
        if code_hash.is_zero() {
            return Ok(None);
        }

        if *code_hash == KECCAK_EMPTY {
            debug!("empty code hash");
            return Ok(None);
        }

        if let Some(bytecode) = self.bytecode_cache.get(code_hash) {
            debug!("code hash cache hit");
            return Ok(Some(bytecode.clone()));
        }

        let future = self
            .remote_provider
            .client()
            .request::<_, Bytes>("rbuilder_getCodeByHash", (code_hash,));

        let bytes = self
            .future_runner
            .run(future)
            .map_err(transport_to_provider_error)?;

        let bytecode = Bytecode::new_raw(bytes);

        self.bytecode_cache.insert(*code_hash, bytecode.clone());

        Ok(Some(bytecode))
    }
}

impl<T> BlockHashReader for RemoteStateProvider<T>
where
    T: Transport + Clone,
{
    /// Get the hash of the block with the given number. Returns `None` if no block with this number exists
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        if let Some(hash) = self.block_hash_cache.get(&number) {
            return Ok(Some(*hash));
        }
        let future = self
            .remote_provider
            .client()
            .request::<_, B256>("rbuilder_getBlockHash", (BlockNumberOrTag::Number(number),));
        let block_hash = self
            .future_runner
            .run(future)
            .map_err(transport_to_provider_error)?;

        self.block_hash_cache.insert(number, block_hash);
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

impl<T> AccountReader for RemoteStateProvider<T>
where
    T: Transport + Clone,
{
    /// Get basic account information.
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        //return Ok(Some(Account::default()));

        if let Some(account) = self.account_cache.get(address) {
            return Ok(Some(*account));
        }

        let future = self
            .remote_provider
            .client()
            .request::<_, AccountState>("rbuilder_getAccount", (*address, self.block_id));

        let account = match self.future_runner.run(future) {
            Ok(a) => a,
            Err(e) => {
                println!("error: {e}, address {address}");
                return Err(transport_to_provider_error(e));
            }
        };

        let account = Account {
            nonce: account.nonce.try_into().unwrap(),
            bytecode_hash: account.code_hash.into(),
            balance: account.balance,
        };

        self.account_cache.insert(*address, account);

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

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
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

impl<T> HashedPostStateProvider for RemoteStateProvider<T>
where
    T: Transport + Clone,
{
    fn hashed_post_state(&self, _bundle_state: &BundleState) -> HashedPostState {
        unimplemented!("hashed_post_state not used in remote provider")
    }
}

#[derive(Debug)]
pub struct StatRootHashCalculator<T> {
    remote_provider: RootProvider<T>,
    future_runner: FutureRunner,
    parent_hash: B256,
}

impl<T> RootHasher for StatRootHashCalculator<T>
where
    T: Transport + Clone + Debug,
{
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
        //return Ok(B256::default());
        //
        //println!("state root");
        let account_diff: HashMap<Address, AccountDiff> = outcome
            .bundle
            .state
            .iter()
            .map(|(address, diff)| (*address, diff.clone().into()))
            .collect();

        let future = self.remote_provider.client().request::<_, B256>(
            "rbuilder_calculateStateRoot",
            (BlockId::Hash(self.parent_hash.into()), account_diff),
        );

        let hash = self
            .future_runner
            .run(future)
            .map_err(|_| crate::roothash::RootHashError::Verification)?;

        Ok(hash)
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

#[derive(Clone, Debug)]
struct FutureRunner {
    runtime_handle: tokio::runtime::Handle,
}

impl FutureRunner {
    /// Creates new instance of  FutureRunner
    /// IMPORTANT: MUST be called from within tokio context, otherwise will panic
    fn new() -> Self {
        Self {
            runtime_handle: tokio::runtime::Handle::current(),
        }
    }

    /// Runs fututre in sync context
    // StateProvider(Factory) traits require sync context, but calls to remote provider are async
    // What's more, rbuilder is executed in async context, so we have situation
    // async -> sync -> async
    // This helper function allows execution in such environment
    fn run<F, R>(&self, f: F) -> R
    where
        F: Future<Output = R>,
    {
        tokio::task::block_in_place(move || self.runtime_handle.block_on(f))
    }
}

//TODO: this is temp hack, fix it properly
fn transport_to_provider_error(transport_error: TransportError) -> ProviderError {
    ProviderError::Database(reth_db::DatabaseError::Other(transport_error.to_string()))
}
