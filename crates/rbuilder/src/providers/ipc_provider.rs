use std::ops::RangeBounds;

use alloy_consensus::Header;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{
    map::{HashMap, HashSet},
    BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, U256,
};
use reth_chainspec::ChainInfo;
use reth_errors::ProviderResult;
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

pub struct IPCProviderFactory;
pub struct IPCStateProvider;

impl StateProviderFactory for IPCProviderFactory {
    /// Storage provider for latest block.
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        unimplemented!()
    }

    fn state_by_block_number_or_tag(
        &self,
        _number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        unimplemented!()
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> ProviderResult<StateProviderBox> {
        unimplemented!()
    }

    /// Returns a historical [StateProvider] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        todo!()
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
impl BlockIdReader for IPCProviderFactory {
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
impl BlockNumReader for IPCProviderFactory {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        unimplemented!()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        unimplemented!()
    }

    /// Returns the last block number associated with the last canonical header in the database.
    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        todo!()
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        unimplemented!()
    }
}
// Required by the BlockNumReader
impl BlockHashReader for IPCProviderFactory {
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

impl HeaderProvider for IPCProviderFactory {
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

impl StateProvider for IPCStateProvider {
    /// Get storage of given account
    fn storage(
        &self,
        _account: Address,
        _storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        todo!()
    }

    ///Get account code by its hash
    fn bytecode_by_hash(&self, _code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        todo!()
    }
}

// Required by the StateProvider
impl BlockHashReader for IPCStateProvider {
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
impl AccountReader for IPCStateProvider {
    /// Get basic account information.
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, _address: Address) -> ProviderResult<Option<Account>> {
        todo!()
    }
}

impl StateRootProvider for IPCStateProvider {
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

impl StorageRootProvider for IPCStateProvider {
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

impl StateProofProvider for IPCStateProvider {
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
