use alloy_eips::{BlockHashOrNumber, BlockNumberOrTag};
use alloy_primitives::{BlockHash, BlockNumber, StorageKey, StorageValue, TxHash, TxNumber, U256};
use alloy_rpc_types::Withdrawal;
use reth_chainspec::ChainInfo;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    models::StoredBlockBodyIndices,
    table::{DupSort, Table, TableImporter},
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_errors::ProviderResult;
use reth_primitives::{
    Account, Block, BlockWithSenders, Bytecode, Header, Receipt, SealedBlock,
    SealedBlockWithSenders, SealedHeader, TransactionSigned, TransactionSignedNoHash, Withdrawals,
};
use reth_provider::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockSource,
    DBProvider, DatabaseProviderFactory, HeaderProvider, ReceiptProvider, StateProofProvider,
    StateProvider, StateProviderBox, StateProviderFactory, StateRootProvider, StorageRootProvider,
    TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof, StorageProof,
    TrieInput,
};
use revm_primitives::{Address, Bytes, HashMap, HashSet, B256};
use std::ops::{RangeBounds, RangeInclusive};

#[derive(Debug, Clone)]
pub struct IpcProvider();

// =========== HEADER PROVIDER =========== //
//Required by LiveBuilder
//
//impl<P, DB, BlocksSourceType: SlotSource> LiveBuilder<P, DB, BlocksSourceType>
//where
//    DB: Database + Clone + 'static,
//    P: DatabaseProviderFactory<DB = DB, Provider: BlockReader>
//        + StateProviderFactory
//        + HeaderProvider
//        + Clone
//        + 'static,
//    BlocksSourceType: SlotSource,
impl HeaderProvider for IpcProvider {
    fn header(&self, _block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        todo!()
    }

    fn header_by_number(&self, _num: u64) -> ProviderResult<Option<Header>> {
        todo!()
    }

    fn header_td(&self, _hash: &BlockHash) -> ProviderResult<Option<U256>> {
        todo!()
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> ProviderResult<Option<U256>> {
        todo!()
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        todo!()
    }

    fn sealed_header(&self, _number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        todo!()
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        todo!()
    }
}

// =========== END HEADER PROVIDER =========== //

// =========== STATE PROVIDER FACTORY ========== //
//Required by LiveBuilder
//
//impl<P, DB, BlocksSourceType: SlotSource> LiveBuilder<P, DB, BlocksSourceType>
//where
//    DB: Database + Clone + 'static,
//    P: DatabaseProviderFactory<DB = DB, Provider: BlockReader>
//        + StateProviderFactory
//        + HeaderProvider
//        + Clone
//        + 'static,
//    BlocksSourceType: SlotSource,
impl StateProviderFactory for IpcProvider {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn state_by_block_number_or_tag(
        &self,
        _number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        todo!()
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        todo!()
    }
}

// =========== STATE PROVIDER FACTORY ========== //

// ======= DATABASE PROVIDER FACTORY ======= //

impl DatabaseProviderFactory for IpcProvider {
    type DB = MockDB;
    type Provider = IpcProvider;
    type ProviderRW = IpcProvider;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        todo!()
    }

    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
        todo!()
    }
}

impl DBProvider for IpcProvider {
    type Tx = DbTxMock;

    fn tx_ref(&self) -> &Self::Tx {
        todo!()
    }

    fn tx_mut(&mut self) -> &mut Self::Tx {
        todo!()
    }

    fn into_tx(self) -> Self::Tx {
        todo!()
    }

    fn prune_modes_ref(&self) -> &reth_prune_types::PruneModes {
        todo!()
    }
}

#[derive(Debug, Clone)]
pub struct MockDB();

//Required by DatabaseProviderFactory (type DB: Database)
impl Database for MockDB {
    type TX = DbTxMock;

    type TXMut = DbTxMock;

    fn tx(&self) -> Result<Self::TX, reth_db::DatabaseError> {
        todo!()
    }

    fn tx_mut(&self) -> Result<Self::TXMut, reth_db::DatabaseError> {
        todo!()
    }
}
#[derive(Debug, Clone)]
pub struct DbTxMock();

// Required by DbTxMut, note `reth` provides blanket implementations
impl TableImporter for DbTxMock {}

// Required by Database trait
impl DbTx for DbTxMock {
    type Cursor<T: reth_db::table::Table> = CursorMock;

    type DupCursor<T: reth_db::table::DupSort> = CursorMock;

    fn get<T: reth_db::table::Table>(
        &self,
        _key: T::Key,
    ) -> Result<Option<T::Value>, reth_db::DatabaseError> {
        todo!()
    }

    fn commit(self) -> Result<bool, reth_db::DatabaseError> {
        todo!()
    }

    fn abort(self) {
        todo!()
    }

    fn cursor_read<T: reth_db::table::Table>(
        &self,
    ) -> Result<Self::Cursor<T>, reth_db::DatabaseError> {
        todo!()
    }

    fn cursor_dup_read<T: reth_db::table::DupSort>(
        &self,
    ) -> Result<Self::DupCursor<T>, reth_db::DatabaseError> {
        todo!()
    }

    fn entries<T: reth_db::table::Table>(&self) -> Result<usize, reth_db::DatabaseError> {
        todo!()
    }

    fn disable_long_read_transaction_safety(&mut self) {
        todo!()
    }
}

// Required by Database trait
impl DbTxMut for DbTxMock {
    type CursorMut<T: reth_db::table::Table> = CursorMock;

    type DupCursorMut<T: reth_db::table::DupSort> = CursorMock;

    fn put<T: reth_db::table::Table>(
        &self,
        _key: T::Key,
        _value: T::Value,
    ) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }

    fn delete<T: reth_db::table::Table>(
        &self,
        _key: T::Key,
        _value: Option<T::Value>,
    ) -> Result<bool, reth_db::DatabaseError> {
        todo!()
    }

    fn clear<T: reth_db::table::Table>(&self) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }

    fn cursor_write<T: reth_db::table::Table>(
        &self,
    ) -> Result<Self::CursorMut<T>, reth_db::DatabaseError> {
        todo!()
    }

    fn cursor_dup_write<T: reth_db::table::DupSort>(
        &self,
    ) -> Result<Self::DupCursorMut<T>, reth_db::DatabaseError> {
        todo!()
    }
}

pub struct CursorMock();

impl<T: Table> DbCursorRO<T> for CursorMock {
    fn first(&mut self) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn seek_exact(&mut self, _key: T::Key) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn seek(&mut self, _key: T::Key) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn next(&mut self) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn prev(&mut self) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn last(&mut self) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn current(&mut self) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn walk(
        &mut self,
        _start_key: Option<T::Key>,
    ) -> Result<reth_db::cursor::Walker<'_, T, Self>, reth_db::DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_range(
        &mut self,
        _range: impl RangeBounds<T::Key>,
    ) -> Result<reth_db::cursor::RangeWalker<'_, T, Self>, reth_db::DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_back(
        &mut self,
        _start_key: Option<T::Key>,
    ) -> Result<reth_db::cursor::ReverseWalker<'_, T, Self>, reth_db::DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl<T: DupSort> DbDupCursorRO<T> for CursorMock {
    fn next_dup(&mut self) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn next_no_dup(&mut self) -> reth_db::common::PairResult<T> {
        todo!()
    }

    fn next_dup_val(&mut self) -> reth_db::common::ValueOnlyResult<T> {
        todo!()
    }

    fn seek_by_key_subkey(
        &mut self,
        _key: T::Key,
        _subkey: T::SubKey,
    ) -> reth_db::common::ValueOnlyResult<T> {
        todo!()
    }

    fn walk_dup(
        &mut self,
        _key: Option<T::Key>,
        _subkey: Option<T::SubKey>,
    ) -> Result<reth_db::cursor::DupWalker<'_, T, Self>, reth_db::DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl<T: DupSort> DbDupCursorRW<T> for CursorMock {
    fn delete_current_duplicates(&mut self) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }

    fn append_dup(&mut self, _key: T::Key, _value: T::Value) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }
}

impl<T: Table> DbCursorRW<T> for CursorMock {
    fn upsert(&mut self, _key: T::Key, _value: T::Value) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }

    fn insert(&mut self, _key: T::Key, _value: T::Value) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }

    fn append(&mut self, _key: T::Key, _value: T::Value) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }

    fn delete_current(&mut self) -> Result<(), reth_db::DatabaseError> {
        todo!()
    }
}

// ======= END DATABASE PROVIDER FACTORY ======= //

// =========== BLOCK READERS =========== //
// Required by StateProviderFactory
impl BlockIdReader for IpcProvider {
    fn pending_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        todo!()
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        todo!()
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        todo!()
    }
}

// Required by BlockIdReader
impl BlockNumReader for IpcProvider {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        todo!()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        todo!()
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        todo!()
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        todo!()
    }
}

// Requires by BlockNumReader
impl BlockHashReader for IpcProvider {
    fn block_hash(&self, _number: BlockNumber) -> ProviderResult<Option<B256>> {
        todo!()
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        todo!()
    }
}

// =========== END BLOCK READERS =========== //

// ========== BLOCK READER ================//
impl BlockReader for IpcProvider {
    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Block>> {
        todo!()
    }

    fn block(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        todo!()
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        todo!()
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        todo!()
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        todo!()
    }
    fn ommers(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        todo!()
    }

    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        todo!()
    }

    fn block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        todo!()
    }

    fn sealed_block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        todo!()
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        todo!()
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        todo!()
    }

    fn sealed_block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        todo!()
    }
}

impl TransactionsProvider for IpcProvider {
    fn transaction_id(&self, _tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        todo!()
    }

    fn transaction_by_id(&self, _id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        todo!()
    }

    fn transaction_by_id_no_hash(
        &self,
        _id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        todo!()
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        todo!()
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, reth_primitives::TransactionMeta)>> {
        todo!()
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        todo!()
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        todo!()
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<TransactionSignedNoHash>> {
        todo!()
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        todo!()
    }

    fn transaction_sender(&self, _id: TxNumber) -> ProviderResult<Option<Address>> {
        todo!()
    }
}

impl ReceiptProvider for IpcProvider {
    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Receipt>> {
        todo!()
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Receipt>> {
        todo!()
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        todo!()
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        todo!()
    }
}

impl WithdrawalsProvider for IpcProvider {
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        todo!()
    }

    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        todo!()
    }
}

// ========== END BLOCK READER ================//

impl StateProvider for IpcProvider {
    fn storage(
        &self,
        _account: Address,
        _storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        todo!()
    }

    fn bytecode_by_hash(&self, _code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        todo!()
    }
}

impl AccountReader for IpcProvider {
    fn basic_account(&self, _address: Address) -> ProviderResult<Option<Account>> {
        todo!()
    }
}

impl StateRootProvider for IpcProvider {
    fn state_root(&self, _hashed_state: HashedPostState) -> ProviderResult<B256> {
        todo!()
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        todo!()
    }

    fn state_root_with_updates(
        &self,
        _hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        todo!()
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        todo!()
    }
}

impl StorageRootProvider for IpcProvider {
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        todo!()
    }

    fn storage_proof(
        &self,
        _address: Address,
        _slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        todo!()
    }
}

impl StateProofProvider for IpcProvider {
    fn proof(
        &self,
        _input: TrieInput,
        _address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        todo!()
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: HashMap<B256, HashSet<B256>>,
    ) -> ProviderResult<MultiProof> {
        todo!()
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        todo!()
    }
}
