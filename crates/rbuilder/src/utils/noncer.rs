use ahash::HashMap;
use alloy_primitives::{Address, B256};
use reth_errors::ProviderResult;
use reth_provider::{StateProviderBox, StateProviderFactory};
use std::sync::{Arc, Mutex};

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
    cache: Arc<Mutex<HashMap<Address, u64>>>,
    block: B256,
}

impl<P> NonceCache<P>
where
    P: NonceStateProvider,
{
    pub fn new(provider: P, block: B256) -> Self {
        Self {
            provider,
            cache: Arc::new(Mutex::new(HashMap::default())),
            block,
        }
    }

    pub fn get_ref(&self) -> ProviderResult<NonceCacheRef<P::Provider>> {
        let state = self.provider.get_state_by_block_hash(self.block)?;
        Ok(NonceCacheRef {
            state,
            cache: Arc::clone(&self.cache),
        })
    }
}

pub struct NonceCacheRef<P> {
    state: P,
    cache: Arc<Mutex<HashMap<Address, u64>>>,
}

impl<P> NonceCacheRef<P>
where
    P: NonceProvider,
{
    pub fn nonce(&self, address: Address) -> ProviderResult<u64> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(nonce) = cache.get(&address) {
            return Ok(*nonce);
        }
        let nonce = self.state.get_nonce(address)?.unwrap_or_default();
        cache.insert(address, nonce);
        Ok(nonce)
    }
}

pub trait NonceProvider {
    fn get_nonce(&self, address: Address) -> ProviderResult<Option<u64>>;
}

pub trait NonceStateProvider {
    type Provider: NonceProvider;

    fn get_state_by_block_hash(&self, block: B256) -> ProviderResult<Self::Provider>;
}

impl<T> NonceStateProvider for T
where
    T: StateProviderFactory,
{
    type Provider = StateProviderBox;

    fn get_state_by_block_hash(&self, block: B256) -> ProviderResult<Self::Provider> {
        self.history_by_block_hash(block)
    }
}

impl NonceProvider for StateProviderBox {
    fn get_nonce(&self, address: Address) -> ProviderResult<Option<u64>> {
        self.account_nonce(address)
    }
}
