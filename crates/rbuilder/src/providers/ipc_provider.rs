use std::ops::RangeBounds;

use alloy_consensus::Header;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{BlockHash, BlockNumber, U256};
use reth_chainspec::ChainInfo;
use reth_errors::ProviderResult;
use reth_primitives::SealedHeader;
use reth_provider::{
    BlockHashReader, BlockIdReader, BlockNumReader, HeaderProvider, StateProviderBox,
    StateProviderFactory,
};
use revm_primitives::B256;

pub struct IpcProviderFactory;

impl StateProviderFactory for IpcProviderFactory {
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
impl BlockIdReader for IpcProviderFactory {
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
impl BlockNumReader for IpcProviderFactory {
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
impl BlockHashReader for IpcProviderFactory {
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

impl HeaderProvider for IpcProviderFactory {
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
