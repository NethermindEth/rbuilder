use crate::provider::StateProviderFactory;
use ahash::HashMap;
use alloy_primitives::{Address, B256};
use dashmap::DashMap;
use parking_lot::Mutex;
use reth::providers::StateProviderBox;
use reth_errors::ProviderResult;
use std::sync::Arc;
use tracing::{debug, debug_span};

/// Struct to get nonces for Addresses, caching the results.
/// NonceCache contains the data (but doesn't allow you to query it) and NonceCacheRef is a reference that allows you to query it.
/// Usage:
/// - Create a NonceCache
/// - For every context where the nonce is needed call NonceCache::get_ref and call NonceCacheRef::nonce all the times you need.
///   Neither NonceCache or NonceCacheRef are clonable, the clone of shared info happens on get_ref where we clone the internal cache.
#[derive(Debug)]
pub struct NonceCache<P> {
    provider: P,
    // We have to use Arc<Mutex here because Rc are not Send (so can't be used in futures)
    // and borrows don't work when nonce cache is a field in a struct.
    cache: Arc<DashMap<Address, u64>>,
    block: B256,
}

impl<P> NonceCache<P>
where
    P: StateProviderFactory,
{
    pub fn new(provider: P, block: B256) -> Self {
        Self {
            provider,
            cache: Arc::new(DashMap::new()),
            block,
        }
    }

    pub fn get_ref(&self) -> ProviderResult<NonceCacheRef> {
        let state = self.provider.history_by_block_hash(self.block)?;
        Ok(NonceCacheRef {
            state,
            cache: Arc::clone(&self.cache),
        })
    }
}

pub struct NonceCacheRef {
    state: StateProviderBox,
    cache: Arc<DashMap<Address, u64>>,
}

impl NonceCacheRef {
    pub fn nonce(&self, address: Address) -> ProviderResult<u64> {
        let nonce: u64 = rand::random();
        //let id: u64 = rand::random();
        //let span = debug_span!("noncer", id, address = address.to_string());
        //let _guard = span.enter();
        //debug!("noncer: get");
        //
        //if let Some(nonce) = self.cache.get(&address) {
        //    debug!("noncer: cache hit");
        //    return Ok(*nonce);
        //}

        let nonce: u64 = rand::random();
        //let nonce = self.state.account_nonce(&address)?.unwrap_or_default();
        //self.cache.insert(address, nonce);
        //debug!("noncer: got it");
        Ok(nonce)
    }
}
