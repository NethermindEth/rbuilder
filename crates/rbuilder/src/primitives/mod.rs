//! Order types used as elements for block building.

pub mod fmt;
pub mod mev_boost;
pub mod order_builder;
pub mod serialize;
mod test_data_generator;

use crate::building::evm_inspector::UsedStateTrace;
use alloy_consensus::Transaction as _;
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Encodable2718},
    eip4844::{Blob, BlobTransactionSidecar, Bytes48},
    Typed2718,
};
use alloy_primitives::{keccak256, Address, Bytes, TxHash, B256, U256};
use derivative::Derivative;
use integer_encoding::VarInt;
use reth::transaction_pool::{
    BlobStore, BlobStoreError, EthPooledTransaction, Pool, TransactionOrdering, TransactionPool,
    TransactionValidator,
};
use reth_node_core::primitives::SignedTransaction;
use reth_primitives::{
    kzg::{BYTES_PER_BLOB, BYTES_PER_COMMITMENT, BYTES_PER_PROOF},
    transaction::SignedTransactionIntoRecoveredExt,
    PooledTransaction, Recovered, Transaction, TransactionSigned,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{cmp::Ordering, collections::HashMap, fmt::Display, hash::Hash, str::FromStr, sync::Arc};
pub use test_data_generator::TestDataGenerator;
use thiserror::Error;
use uuid::Uuid;

/// Extra metadata for ShareBundle/Bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Metadata {
    pub received_at_timestamp: time::OffsetDateTime,
}

impl Metadata {
    pub fn with_current_received_at() -> Self {
        Self {
            received_at_timestamp: time::OffsetDateTime::now_utc(),
        }
    }
}

impl Default for Metadata {
    fn default() -> Self {
        Self::with_current_received_at()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct AccountNonce {
    pub nonce: u64,
    pub account: Address,
}
impl AccountNonce {
    pub fn with_nonce(self, nonce: u64) -> Self {
        AccountNonce {
            account: self.account,
            nonce,
        }
    }
}

/// BundledTxInfo should replace Nonce in the future.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BundledTxInfo {
    pub nonce: AccountNonce,
    /// optional -> can revert and the bundle continues.
    pub optional: bool,
}

/// @Pending: Delete and replace all uses by BundledTxInfo.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nonce {
    pub nonce: u64,
    pub address: Address,
    pub optional: bool,
}

/// Information regarding a new/update replaceable Bundle/ShareBundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplacementData<KeyType> {
    pub key: KeyType,
    /// Due to simulation async problems Bundle updates can arrive out of order.
    /// sequence_number allows us to keep always the last one.
    pub sequence_number: u64,
}

impl<KeyType: Clone> ReplacementData<KeyType> {
    /// Next sequence_number, useful for testing.
    pub fn next(&self) -> Self {
        Self {
            key: self.key.clone(),
            sequence_number: self.sequence_number + 1,
        }
    }
}

pub type BundleReplacementData = ReplacementData<BundleReplacementKey>;

#[derive(Eq, PartialEq, Clone, Hash, Debug)]
pub struct BundleRefund {
    /// Percent to refund back to the user.
    pub percent: u8,
    /// Address where to refund to.
    pub recipient: Address,
    /// A list of transaction hashes to refund.
    /// This means that part (percent%) of the profit from the execution these txs goes to refund.recipient
    pub tx_hashes: Vec<TxHash>,
}

/// Bundle sent to us usually by a searcher via eth_sendBundle (https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint#eth_sendbundle).
#[derive(Derivative)]
#[derivative(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bundle {
    /// None means in the first possible block.
    pub block: Option<u64>,
    pub min_timestamp: Option<u64>,
    pub max_timestamp: Option<u64>,
    pub txs: Vec<TransactionSignedEcRecoveredWithBlobs>,
    /// A list of tx hashes that are allowed to revert.
    pub reverting_tx_hashes: Vec<B256>,
    /// A list of tx hashes that are allowed to be discarded, but may not revert on chain.
    pub dropping_tx_hashes: Vec<B256>,
    /// Virtual hash generated by concatenating all txs hashes (+some more info) and hashing them.
    /// See [Bundle::hash_slow] for more details.
    pub hash: B256,
    /// Unique id we generate.
    pub uuid: Uuid,
    /// Unique id, bundle signer.
    /// The unique id was generated by the sender and is used for updates/cancellations.
    /// Bundle signer is redundant with self.signer.
    pub replacement_data: Option<BundleReplacementData>,
    pub signer: Option<Address>,

    #[derivative(PartialEq = "ignore", Hash = "ignore")]
    pub metadata: Metadata,

    /// Bundle refund data.
    pub refund: Option<BundleRefund>,
}

impl Bundle {
    pub fn can_execute_with_block_base_fee(&self, block_base_fee: u128) -> bool {
        can_execute_with_block_base_fee(self.list_txs(), block_base_fee)
    }

    /// BundledTxInfo for all the child txs.
    pub fn nonces(&self) -> Vec<Nonce> {
        let txs = self
            .txs
            .iter()
            .map(|tx| (tx, self.reverting_tx_hashes.contains(&tx.hash())));
        bundle_nonces(txs)
    }

    fn list_txs(&self) -> Vec<(&TransactionSignedEcRecoveredWithBlobs, bool)> {
        self.txs
            .iter()
            .map(|tx| (tx, self.reverting_tx_hashes.contains(&tx.hash())))
            .collect()
    }

    /// Returns `true` if the provided transaction hash is refundable.
    /// This means that part the profit from this execution goes to the self.refund.recipient
    pub fn is_tx_refundable(&self, hash: &B256) -> bool {
        self.refund
            .as_ref()
            .map(|r| r.tx_hashes.contains(hash))
            .unwrap_or_default()
    }

    /// Recalculate bundle hash and uuid.
    /// Hash is computed from child tx hashes + reverting_tx_hashes + dropping_tx_hashes.
    /// @Pending: improve since moving txs from reverting_tx_hashes to dropping_tx_hashes would give the same uuid
    pub fn hash_slow(&mut self) {
        let hash = self
            .txs
            .iter()
            .flat_map(|tx| tx.hash().0.to_vec())
            .collect::<Vec<_>>();
        self.hash = keccak256(hash);

        let uuid = {
            // Block, hash, reverting hashes.
            let mut buff = Vec::with_capacity(
                8 + 32 + 32 * (self.reverting_tx_hashes.len() + self.dropping_tx_hashes.len()),
            );
            {
                let block = self.block.unwrap_or_default() as i64;
                buff.append(&mut block.encode_var_vec());
            }
            buff.extend_from_slice(self.hash.as_slice());
            self.reverting_tx_hashes.sort();
            for reverted_hash in &self.reverting_tx_hashes {
                buff.extend_from_slice(reverted_hash.as_slice());
            }
            for dropping_hash in &self.dropping_tx_hashes {
                buff.extend_from_slice(dropping_hash.as_slice());
            }
            let hash = {
                let mut res = [0u8; 16];
                let mut hasher = Sha256::new();
                // We write 16 zeroes to replicate golang hashing behavior.
                hasher.update(res);
                hasher.update(&buff);
                let output = hasher.finalize();
                res.copy_from_slice(&output.as_slice()[0..16]);
                res
            };
            uuid::Builder::from_sha1_bytes(hash).into_uuid()
        };
        self.uuid = uuid;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxRevertBehavior {
    /// Tx in a bundle can't revert.
    NotAllowed,
    /// If the tx reverts it will be included. This is the old "can_revert" boolean.
    AllowedIncluded,
    /// If the tx reverts we will ignore it.
    AllowedExcluded,
}

impl TxRevertBehavior {
    /// Backwards compatibility.
    pub fn from_old_bool(can_revert: bool) -> Self {
        if can_revert {
            TxRevertBehavior::AllowedIncluded
        } else {
            TxRevertBehavior::NotAllowed
        }
    }
    pub fn can_revert(&self) -> bool {
        match self {
            TxRevertBehavior::NotAllowed => false,
            TxRevertBehavior::AllowedIncluded | TxRevertBehavior::AllowedExcluded => true,
        }
    }
}

/// Tx as part of a mev share body.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShareBundleTx {
    pub tx: TransactionSignedEcRecoveredWithBlobs,
    pub revert_behavior: TxRevertBehavior,
}

impl ShareBundleTx {
    pub fn hash(&self) -> TxHash {
        self.tx.hash()
    }
}

/// Body element of a mev share bundle.
/// [`ShareBundleInner::body`] is formed by several of these.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShareBundleBody {
    Tx(ShareBundleTx),
    Bundle(ShareBundleInner),
}

impl ShareBundleBody {
    pub fn refund_config(&self) -> Option<Vec<RefundConfig>> {
        match self {
            Self::Tx(sbundle_tx) => Some(vec![RefundConfig {
                address: sbundle_tx.tx.signer(),
                percent: 100,
            }]),
            Self::Bundle(b) => b.refund_config(),
        }
    }
}

/// Mev share contains 2 types of txs:
/// - User txs: simple txs sent to us to be protected and to give kickbacks to the user.
/// - Searcher txs: Txs added by a searcher to extract MEV from the user txs.
///   Refund points to the user txs on the body and has the kickback percentage for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refund {
    /// Index of the ShareBundleInner::body for which this applies.
    pub body_idx: usize,
    /// Percent of the profit going back to the user as kickback.
    pub percent: usize,
}

/// Users can specify how to get kickbacks and this is propagated by the MEV-Share Node to us.
/// We get this configuration as multiple RefundConfigs, then the refunds are paid to the specified addresses in the indicated percentages.
/// The sum of all RefundConfig::percent on a mev share bundle should be 100%.
/// See [ShareBundleInner::refund_config] for more details.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundConfig {
    pub address: Address,
    pub percent: usize,
}

/// sub bundle as part of a mev share body
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShareBundleInner {
    pub body: Vec<ShareBundleBody>,
    pub refund: Vec<Refund>,
    /// Optional RefundConfig for this ShareBundleInner. see [ShareBundleInner::refund_config] for more details.
    pub refund_config: Vec<RefundConfig>,
    /// We are allowed to skip this sub bundle (either because of inner reverts or any other reason).
    /// Added specifically to allow same user sbundle merging since we stick together many sbundles and allow some of them to fail.
    pub can_skip: bool,
    /// Patch to track the original orders when performing order merging (see [`ShareBundleMerger`]).
    pub original_order_id: Option<OrderId>,
}

impl ShareBundleInner {
    fn list_txs(&self) -> Vec<(&TransactionSignedEcRecoveredWithBlobs, bool)> {
        self.body
            .iter()
            .flat_map(|b| match b {
                ShareBundleBody::Tx(sbundle_tx) => {
                    vec![(&sbundle_tx.tx, sbundle_tx.revert_behavior.can_revert())]
                }
                ShareBundleBody::Bundle(bundle) => bundle.list_txs(),
            })
            .collect()
    }

    /// Refunds config for the ShareBundleInner.
    /// refund_config not empty -> we use it
    /// refund_config empty:
    ///     - body empty (illegal?) -> None
    ///     - body not empty -> first child refund_config()
    /// Since for ShareBundleBody::Tx we use 100% to the signer of the tx (see [ShareBundleBody::refund_config]) as RefundConfig this basically
    /// makes DFS looking for the first ShareBundleInner with explicit RefundConfig or the first Tx.
    pub fn refund_config(&self) -> Option<Vec<RefundConfig>> {
        if !self.refund_config.is_empty() {
            return Some(self.refund_config.clone());
        }
        if self.body.is_empty() {
            return None;
        }
        self.body[0].refund_config()
    }

    // Recalculate bundle hash.
    pub fn hash_slow(&self) -> B256 {
        let hashes = self
            .body
            .iter()
            .map(|b| match b {
                ShareBundleBody::Tx(sbundle_tx) => sbundle_tx.tx.hash(),
                ShareBundleBody::Bundle(inner) => inner.hash_slow(),
            })
            .collect::<Vec<_>>();
        if hashes.len() == 1 {
            hashes[0]
        } else {
            keccak256(
                hashes
                    .into_iter()
                    .flat_map(|h| h.0.to_vec())
                    .collect::<Vec<_>>(),
            )
        }
    }
}

/// Uniquely identifies a replaceable sbundle or bundle
#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub struct ReplacementKey {
    pub id: Uuid,
    /// None means we don't have signer so the identity will be only by uuid.
    /// Source not giving signer risk uuid collision but if uuid is properly generated is almost impossible.
    pub signer: Option<Address>,
}

pub type ShareBundleReplacementData = ReplacementData<ShareBundleReplacementKey>;

/// Preprocessed Share bundle originated by mev_sendBundle (https://docs.flashbots.net/flashbots-auction/advanced/rpc-endpoint#eth_sendbundle)
/// Instead of having hashes (as in the original definition) it contains the actual txs.
#[derive(Derivative)]
#[derivative(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShareBundle {
    /// Hash for the ShareBundle (also used in OrderId::ShareBundle).
    /// See [ShareBundle::hash_slow] for more details.
    pub hash: B256,
    pub block: u64,
    pub max_block: u64,
    pub inner_bundle: ShareBundleInner,
    pub signer: Option<Address>,
    /// data that uniquely identifies this ShareBundle for update or cancellation
    pub replacement_data: Option<ShareBundleReplacementData>,
    /// Only used internally when we build a virtual (not part of the orderflow) ShareBundle from other orders.
    pub original_orders: Vec<Order>,

    #[derivative(PartialEq = "ignore", Hash = "ignore")]
    pub metadata: Metadata,
}

impl ShareBundle {
    pub fn can_execute_with_block_base_fee(&self, block_base_fee: u128) -> bool {
        can_execute_with_block_base_fee(self.list_txs(), block_base_fee)
    }

    pub fn list_txs(&self) -> Vec<(&TransactionSignedEcRecoveredWithBlobs, bool)> {
        self.inner_bundle.list_txs()
    }

    /// BundledTxInfo for all the child txs
    pub fn nonces(&self) -> Vec<Nonce> {
        bundle_nonces(self.inner_bundle.list_txs().into_iter())
    }

    pub fn flatten_txs(&self) -> Vec<(&TransactionSignedEcRecoveredWithBlobs, bool)> {
        self.inner_bundle.list_txs()
    }

    // Recalculate bundle hash.
    /// Sadly it's not perfect since it only hashes inner txs in DFS, so tree structure and all other cfg is lost.
    pub fn hash_slow(&mut self) {
        self.hash = self.inner_bundle.hash_slow();
    }

    /// Patch to store the original orders for a merged order (see [`ShareBundleMerger`])
    pub fn original_orders(&self) -> Vec<&Order> {
        self.original_orders.iter().collect()
    }

    /// see [`ShareBundleMerger`]
    pub fn is_merged_order(&self) -> bool {
        !self.original_orders.is_empty()
    }
}

/// First idea to handle blobs, might change.
/// Don't like the fact that blobs_sidecar exists no matter if Recovered<TransactionSigned> contains a non blob tx.
/// Great effort was put in avoiding simple access to the internal tx so we don't accidentally leak information on logs (particularly the tx sign).
#[derive(Derivative)]
#[derivative(Clone, PartialEq, Eq)]
pub struct TransactionSignedEcRecoveredWithBlobs {
    tx: Recovered<TransactionSigned>,
    /// Will have a non empty BlobTransactionSidecar if Recovered<TransactionSigned> is 4844
    pub blobs_sidecar: Arc<BlobTransactionSidecar>,

    #[derivative(PartialEq = "ignore", Hash = "ignore")]
    pub metadata: Metadata,
}

impl AsRef<TransactionSigned> for TransactionSignedEcRecoveredWithBlobs {
    fn as_ref(&self) -> &TransactionSigned {
        &self.tx
    }
}

impl Typed2718 for TransactionSignedEcRecoveredWithBlobs {
    fn ty(&self) -> u8 {
        self.tx.ty()
    }
}

impl Encodable2718 for TransactionSignedEcRecoveredWithBlobs {
    fn type_flag(&self) -> Option<u8> {
        self.tx.type_flag()
    }

    fn encode_2718_len(&self) -> usize {
        self.tx.encode_2718_len()
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.tx.encode_2718(out)
    }
}

/// Custom fmt to avoid leaking information.
impl std::fmt::Debug for TransactionSignedEcRecoveredWithBlobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TransactionSignedEcRecoveredWithBlobs {{ hash: {} }}",
            self.hash(),
        )
    }
}

#[derive(Error, Debug, derive_more::From)]
pub enum TxWithBlobsCreateError {
    #[error("Failed to decode transaction, error: {0}")]
    FailedToDecodeTransaction(Eip2718Error),
    #[error("Invalid transaction signature")]
    InvalidTransactionSignature,
    #[error("UnexpectedError")]
    UnexpectedError,
    /// This error is generated when we fail (like FailedToDecodeTransaction) parsing in TxEncoding::WithBlobData mode (Network encoding) but the header looks
    /// like the beginning of an ethereum mainnet Canonical encoding 4484 tx.
    /// To avoid consuming resources the generation of this error might not be perfect but helps 99% of the time.
    #[error("Failed to decode transaction, error: {0}. It probably is a 4484 canonical tx.")]
    FailedToDecodeTransactionProbablyIs4484Canonical(alloy_rlp::Error),
    #[error("Tried to create an EIP4844 transaction without a blob")]
    Eip4844MissingBlobSidecar,
    #[error("Tried to create a non-EIP4844 transaction while passing blobs")]
    BlobsMissingEip4844,
    #[error("BlobStoreError: {0}")]
    BlobStore(BlobStoreError),
}

impl TransactionSignedEcRecoveredWithBlobs {
    /// Create new with an optional blob sidecar.
    ///
    /// Warning: It is the caller's responsibility to check if a tx has blobs.
    /// This fn will return an Err if it is passed an eip4844 without blobs,
    /// or blobs without an eip4844.
    pub fn new(
        tx: Recovered<TransactionSigned>,
        blob_sidecar: Option<BlobTransactionSidecar>,
        metadata: Option<Metadata>,
    ) -> Result<Self, TxWithBlobsCreateError> {
        // Check for an eip4844 tx passed without blobs
        if tx.transaction().blob_versioned_hashes().is_some() && blob_sidecar.is_none() {
            Err(TxWithBlobsCreateError::Eip4844MissingBlobSidecar)
        // Check for a non-eip4844 tx passed with blobs
        } else if blob_sidecar.is_some() && tx.transaction().blob_versioned_hashes().is_none() {
            Err(TxWithBlobsCreateError::BlobsMissingEip4844)
        // Groovy!
        } else {
            Ok(Self {
                tx,
                blobs_sidecar: Arc::new(blob_sidecar.unwrap_or_default()),
                metadata: metadata.unwrap_or_default(),
            })
        }
    }

    /// Shorthand for `new(tx, None, None)`
    pub fn new_no_blobs(tx: Recovered<TransactionSigned>) -> Result<Self, TxWithBlobsCreateError> {
        Self::new(tx, None, None)
    }

    /// Try to create a [`TransactionSignedEcRecoveredWithBlobs`] from a
    /// [`Recovered<TransactionSigned>`] and reth pool.
    ///
    /// The pool is required because [`Recovered<TransactionSigned>`] on its
    /// own does not contain blob information, it is required to fetch the blob.
    ///
    /// Unfortunately we need to pass the entire pool, because the blob store
    /// is not part of the pool's public api.
    pub fn try_from_tx_without_blobs_and_pool<V, T, S>(
        tx: Recovered<TransactionSigned>,
        pool: Pool<V, T, S>,
    ) -> Result<Self, TxWithBlobsCreateError>
    where
        V: TransactionValidator<Transaction = EthPooledTransaction>,
        T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
        S: BlobStore,
    {
        let blob_sidecar = pool.get_blob(*tx.tx().hash())?.map(|b| (*b).clone());
        Self::new(tx, blob_sidecar, None)
    }

    /// Creates a Self with empty blobs sidecar. No consistency check is performed!
    pub fn new_for_testing(tx: Recovered<TransactionSigned>) -> Self {
        Self {
            tx,
            blobs_sidecar: Default::default(),
            metadata: Default::default(),
        }
    }

    pub fn hash(&self) -> TxHash {
        *self.tx.tx().hash()
    }

    pub fn signer(&self) -> Address {
        self.tx.signer()
    }

    pub fn to(&self) -> Option<Address> {
        self.tx.to()
    }

    pub fn nonce(&self) -> u64 {
        self.tx.nonce()
    }

    pub fn value(&self) -> U256 {
        self.tx.value()
    }

    /// USE CAREFULLY since this exposes the signed tx.
    pub fn internal_tx_unsecure(&self) -> &Recovered<TransactionSigned> {
        &self.tx
    }

    /// USE CAREFULLY since this exposes the signed tx.
    pub fn into_internal_tx_unsecure(self) -> Recovered<TransactionSigned> {
        self.tx
    }

    /// Encodes the "raw" canonical format of transaction (NOT the one used in `eth_sendRawTransaction`) BLOB DATA IS NOT ENCODED.
    /// I intentsionally omitted the version with blob data since we don't use it and may lead to confusions/bugs.
    /// USE CAREFULLY since this exposes the signed tx.
    pub fn envelope_encoded_no_blobs(&self) -> Bytes {
        let mut buf = Vec::new();
        self.tx.encode_2718(&mut buf);
        buf.into()
    }

    /// Decodes the "raw" format of transaction (e.g. `eth_sendRawTransaction`) with the blob data (network format)
    pub fn decode_enveloped_with_real_blobs(
        raw_tx: Bytes,
    ) -> Result<TransactionSignedEcRecoveredWithBlobs, TxWithBlobsCreateError> {
        let raw_tx = &mut raw_tx.as_ref();
        let pooled_tx = PooledTransaction::decode_2718(raw_tx)
            .map_err(TxWithBlobsCreateError::FailedToDecodeTransaction)?;
        let signer = pooled_tx
            .recover_signer()
            .map_err(|_| TxWithBlobsCreateError::InvalidTransactionSignature)?;
        match pooled_tx {
            PooledTransaction::Legacy(_)
            | PooledTransaction::Eip2930(_)
            | PooledTransaction::Eip1559(_)
            | PooledTransaction::Eip7702(_) => {
                let tx_signed = TransactionSigned::from(pooled_tx);
                TransactionSignedEcRecoveredWithBlobs::new_no_blobs(tx_signed.with_signer(signer))
            }
            PooledTransaction::Eip4844(blob_tx) => {
                let (blob_tx, signature, hash) = blob_tx.into_parts();
                let (blob_tx, sidecar) = blob_tx.into_parts();
                let tx_signed =
                    TransactionSigned::new(Transaction::Eip4844(blob_tx), signature, hash);
                Ok(TransactionSignedEcRecoveredWithBlobs {
                    tx: tx_signed.with_signer(signer),
                    blobs_sidecar: Arc::new(sidecar),
                    metadata: Metadata::default(),
                })
            }
        }
    }
    /// Decodes the "raw" canonical format of transaction (NOT the one used in `eth_sendRawTransaction`) generating fake blob data for backtesting
    pub fn decode_enveloped_with_fake_blobs(
        raw_tx: Bytes,
    ) -> Result<TransactionSignedEcRecoveredWithBlobs, TxWithBlobsCreateError> {
        let decoded = TransactionSigned::decode_2718(&mut raw_tx.as_ref())
            .map_err(TxWithBlobsCreateError::FailedToDecodeTransaction)?;
        let tx = decoded
            .try_into_recovered()
            .map_err(|_| TxWithBlobsCreateError::InvalidTransactionSignature)?;
        let mut fake_sidecar = BlobTransactionSidecar::default();
        for _ in 0..tx.blob_versioned_hashes().map_or(0, |hashes| hashes.len()) {
            fake_sidecar.blobs.push(Blob::from([0u8; BYTES_PER_BLOB]));
            fake_sidecar
                .commitments
                .push(Bytes48::from([0u8; BYTES_PER_COMMITMENT]));
            fake_sidecar
                .proofs
                .push(Bytes48::from([0u8; BYTES_PER_PROOF]));
        }
        Ok(TransactionSignedEcRecoveredWithBlobs {
            tx,
            blobs_sidecar: Arc::new(fake_sidecar),
            metadata: Metadata::default(),
        })
    }
}

impl std::hash::Hash for TransactionSignedEcRecoveredWithBlobs {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        //This is enough to identify the tx
        self.tx.hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MempoolTx {
    pub tx_with_blobs: TransactionSignedEcRecoveredWithBlobs,
}

impl MempoolTx {
    pub fn new(tx_with_blobs: TransactionSignedEcRecoveredWithBlobs) -> Self {
        Self { tx_with_blobs }
    }
}

/// Main type used for block building, we build blocks as sequences of Orders
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Order {
    Bundle(Bundle),
    Tx(MempoolTx),
    ShareBundle(ShareBundle),
}

/// Uniquely identifies a replaceable sbundle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShareBundleReplacementKey(ReplacementKey);
impl ShareBundleReplacementKey {
    pub fn new(id: Uuid, signer: Address) -> Self {
        Self(ReplacementKey {
            id,
            signer: Some(signer),
        })
    }

    pub fn key(&self) -> ReplacementKey {
        self.0
    }
}

/// Uniquely identifies a replaceable bundle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BundleReplacementKey(ReplacementKey);
impl BundleReplacementKey {
    pub fn new(id: Uuid, signer: Option<Address>) -> Self {
        Self(ReplacementKey { id, signer })
    }
    pub fn key(&self) -> ReplacementKey {
        self.0
    }
}

/// General type for both BundleReplacementKey and ShareBundleReplacementKey
/// Even although BundleReplacementKey and ShareBundleReplacementKey have the same info they are kept
/// as different types to avoid bugs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OrderReplacementKey {
    Bundle(BundleReplacementKey),
    ShareBundle(ShareBundleReplacementKey),
}

impl Order {
    /// Partial execution is valid as long as some tx is left.
    pub fn can_execute_with_block_base_fee(&self, block_base_fee: u128) -> bool {
        match self {
            Order::Bundle(bundle) => bundle.can_execute_with_block_base_fee(block_base_fee),
            Order::Tx(tx) => tx.tx_with_blobs.tx.max_fee_per_gas() >= block_base_fee,
            Order::ShareBundle(bundle) => bundle.can_execute_with_block_base_fee(block_base_fee),
        }
    }

    /// Patch to allow virtual orders not originated from a source.
    /// This patch allows to easily implement sbundle merging see ([`ShareBundleMerger`]) and keep the original
    /// orders for post execution work (eg: logs).
    /// Non virtual orders should return self
    pub fn original_orders(&self) -> Vec<&Order> {
        match self {
            Order::Bundle(_) => vec![self],
            Order::Tx(_) => vec![self],
            Order::ShareBundle(sb) => {
                let res = sb.original_orders();
                if res.is_empty() {
                    //fallback to this order
                    vec![self]
                } else {
                    res
                }
            }
        }
    }

    /// BundledTxInfo for all the child txs
    pub fn nonces(&self) -> Vec<Nonce> {
        match self {
            Order::Bundle(bundle) => bundle.nonces(),
            Order::Tx(tx) => vec![Nonce {
                nonce: tx.tx_with_blobs.tx.nonce(),
                address: tx.tx_with_blobs.tx.signer(),
                optional: false,
            }],
            Order::ShareBundle(bundle) => bundle.nonces(),
        }
    }

    pub fn id(&self) -> OrderId {
        match self {
            Order::Bundle(bundle) => OrderId::Bundle(bundle.uuid),
            Order::Tx(tx) => OrderId::Tx(tx.tx_with_blobs.hash()),
            Order::ShareBundle(bundle) => OrderId::ShareBundle(bundle.hash),
        }
    }

    pub fn is_tx(&self) -> bool {
        matches!(self, Order::Tx(_))
    }

    /// Vec<(Tx, allowed to revert)>
    pub fn list_txs(&self) -> Vec<(&TransactionSignedEcRecoveredWithBlobs, bool)> {
        match self {
            Order::Bundle(bundle) => bundle.list_txs(),
            Order::Tx(tx) => vec![(&tx.tx_with_blobs, true)],
            Order::ShareBundle(bundle) => bundle.list_txs(),
        }
    }

    pub fn replacement_key(&self) -> Option<OrderReplacementKey> {
        self.replacement_key_and_sequence_number()
            .map(|(key, _)| key)
    }

    pub fn replacement_key_and_sequence_number(&self) -> Option<(OrderReplacementKey, u64)> {
        match self {
            Order::Bundle(bundle) => bundle.replacement_data.as_ref().map(|r| {
                (
                    OrderReplacementKey::Bundle(r.clone().key),
                    r.sequence_number,
                )
            }),
            Order::Tx(_) => None,
            Order::ShareBundle(sbundle) => sbundle.replacement_data.as_ref().map(|r| {
                (
                    OrderReplacementKey::ShareBundle(r.clone().key),
                    r.sequence_number,
                )
            }),
        }
    }

    pub fn has_blobs(&self) -> bool {
        self.list_txs()
            .iter()
            .any(|(tx, _)| !tx.blobs_sidecar.blobs.is_empty())
    }

    pub fn target_block(&self) -> Option<u64> {
        match self {
            Order::Bundle(bundle) => bundle.block,
            Order::Tx(_) => None,
            Order::ShareBundle(bundle) => Some(bundle.block),
        }
    }

    /// Address that signed the bundle request
    pub fn signer(&self) -> Option<Address> {
        match self {
            Order::Bundle(bundle) => bundle.signer,
            Order::ShareBundle(bundle) => bundle.signer,
            Order::Tx(_) => None,
        }
    }

    pub fn metadata(&self) -> &Metadata {
        match self {
            Order::Bundle(bundle) => &bundle.metadata,
            Order::Tx(tx) => &tx.tx_with_blobs.metadata,
            Order::ShareBundle(bundle) => &bundle.metadata,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SimValue {
    /// profit as coinbase delta after executing an Order
    pub coinbase_profit: U256,
    pub gas_used: u64,
    #[serde(default)]
    pub blob_gas_used: u64,
    /// This is computed as coinbase_profit/gas_used so it includes not only gas tip but also payments made directly to coinbase
    pub mev_gas_price: U256,
    /// Kickbacks paid during simulation as (receiver, amount)
    pub paid_kickbacks: Vec<(Address, U256)>,
}

impl SimValue {
    pub fn new(
        coinbase_profit: U256,
        gas_used: u64,
        blob_gas_used: u64,
        paid_kickbacks: Vec<(Address, U256)>,
    ) -> Self {
        let mev_gas_price = if gas_used != 0 {
            coinbase_profit / U256::from(gas_used)
        } else {
            U256::ZERO
        };
        Self {
            coinbase_profit,
            gas_used,
            blob_gas_used,
            mev_gas_price,
            paid_kickbacks,
        }
    }
}

/// Order simulated (usually on top of block) + SimValue
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedOrder {
    pub order: Order,
    pub sim_value: SimValue,
    /// Info about read/write slots during the simulation to help figure out what the Order is doing.
    pub used_state_trace: Option<UsedStateTrace>,
}

impl SimulatedOrder {
    pub fn id(&self) -> OrderId {
        self.order.id()
    }

    pub fn nonces(&self) -> Vec<Nonce> {
        self.order.nonces()
    }
}

/// Unique OrderId used along the whole builder.
/// Sadly it's not perfect since we still might have some collisions (eg: ShareBundle is the tx tree hash which does not include all the other cfg).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderId {
    Tx(B256),
    Bundle(Uuid),
    ShareBundle(B256),
}

impl OrderId {
    pub fn fixed_bytes(&self) -> B256 {
        match self {
            Self::Tx(hash) | Self::ShareBundle(hash) => *hash,
            Self::Bundle(uuid) => {
                let mut out = [0u8; 32];
                out[0..16].copy_from_slice(uuid.as_bytes());
                B256::new(out)
            }
        }
    }

    /// Returns tx hash if the order is mempool tx
    pub fn tx_hash(&self) -> Option<B256> {
        match self {
            Self::Tx(hash) => Some(*hash),
            _ => None,
        }
    }
}

impl FromStr for OrderId {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(hash_str) = s.strip_prefix("tx:") {
            let hash = B256::from_str(hash_str)?;
            Ok(Self::Tx(hash))
        } else if let Some(id_str) = s.strip_prefix("bundle:") {
            let uuid = Uuid::from_str(id_str)?;
            Ok(Self::Bundle(uuid))
        } else if let Some(hash_str) = s.strip_prefix("sbundle:") {
            let hash = B256::from_str(hash_str)?;
            Ok(Self::ShareBundle(hash))
        } else {
            Err(eyre::eyre!("invalid order id"))
        }
    }
}

/// DON'T CHANGE this since this implements ToString which is used for serialization (deserialization on FromStr above)
impl Display for OrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tx(hash) => write!(f, "tx:{:?}", hash),
            Self::Bundle(uuid) => write!(f, "bundle:{:?}", uuid),
            Self::ShareBundle(hash) => write!(f, "sbundle:{:?}", hash),
        }
    }
}

impl PartialOrd for OrderId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderId {
    fn cmp(&self, other: &Self) -> Ordering {
        fn rank(id: &OrderId) -> usize {
            match id {
                OrderId::Tx(_) => 1,
                OrderId::Bundle(_) => 2,
                OrderId::ShareBundle(_) => 3,
            }
        }

        self.fixed_bytes()
            .cmp(&other.fixed_bytes())
            .then_with(|| rank(self).cmp(&rank(other)))
    }
}

fn bundle_nonces<'a>(
    txs: impl Iterator<Item = (&'a TransactionSignedEcRecoveredWithBlobs, bool)>,
) -> Vec<Nonce> {
    let mut nonces: HashMap<Address, Nonce> = HashMap::new();
    for (tx, optional) in txs.map(|(tx_with_blob, optional)| (&tx_with_blob.tx, optional)) {
        nonces
            .entry(tx.signer())
            .and_modify(|nonce| {
                if nonce.nonce > tx.nonce() {
                    nonce.nonce = tx.nonce();
                    nonce.optional = optional;
                }
            })
            .or_insert(Nonce {
                nonce: tx.nonce(),
                address: tx.signer(),
                optional,
            });
    }
    let mut res = nonces.into_values().collect::<Vec<_>>();
    res.sort_by_key(|nonce| nonce.address);
    res
}

/// Checks that at least one tx can execute and that all mandatory txs can.
fn can_execute_with_block_base_fee<Transaction: AsRef<TransactionSigned>>(
    list_txs: Vec<(Transaction, bool)>,
    block_base_fee: u128,
) -> bool {
    let mut executable_tx_count = 0u32;
    for (tx, opt) in list_txs.iter().map(|(tx, opt)| (tx.as_ref(), opt)) {
        if tx.max_fee_per_gas() >= block_base_fee {
            executable_tx_count += 1;
        } else if !opt {
            return false;
        }
    }
    executable_tx_count > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxLegacy;
    use alloy_primitives::fixed_bytes;
    use reth_primitives::{Transaction, TransactionSigned};
    use revm_primitives::PrimitiveSignature;
    use uuid::uuid;

    #[test]
    /// A bundle with a single optional tx paying enough gas should be considered executable
    fn can_execute_single_optional_tx() {
        let needed_base_gas: u128 = 100000;
        let tx = Recovered::new_unchecked(
            TransactionSigned::new(
                Transaction::Legacy(TxLegacy {
                    gas_price: needed_base_gas,
                    ..Default::default()
                }),
                PrimitiveSignature::test_signature(),
                Default::default(),
            ),
            Address::default(),
        );
        assert!(can_execute_with_block_base_fee(
            vec![(tx, true)],
            needed_base_gas
        ));
    }

    #[test]
    fn test_order_id_json() {
        let id = OrderId::Tx(fixed_bytes!(
            "02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5"
        ));
        let serialized = serde_json::to_string(&id).unwrap();
        assert_eq!(
            serialized,
            r#"{"Tx":"0x02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5"}"#
        );

        let id = OrderId::Bundle(uuid!("5d5bf52c-ac3f-57eb-a3e9-fc01b18ca516"));
        let serialized = serde_json::to_string(&id).unwrap();
        assert_eq!(
            serialized,
            r#"{"Bundle":"5d5bf52c-ac3f-57eb-a3e9-fc01b18ca516"}"#
        );

        let id = OrderId::ShareBundle(fixed_bytes!(
            "02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5"
        ));
        let serialized = serde_json::to_string(&id).unwrap();
        assert_eq!(
            serialized,
            r#"{"ShareBundle":"0x02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5"}"#
        );
    }

    #[test]
    fn test_order_id() {
        let id = "bundle:5d5bf52c-ac3f-57eb-a3e9-fc01b18ca516";
        let parsed = OrderId::from_str(id).unwrap();
        assert_eq!(
            parsed,
            OrderId::Bundle(uuid!("5d5bf52c-ac3f-57eb-a3e9-fc01b18ca516"))
        );
        let serialized = parsed.to_string();
        assert_eq!(serialized, id);
        let fixed_bytes = parsed.fixed_bytes();
        assert_eq!(
            fixed_bytes,
            fixed_bytes!("5d5bf52cac3f57eba3e9fc01b18ca51600000000000000000000000000000000")
        );

        let id = "tx:0x02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5";
        let parsed = OrderId::from_str(id).unwrap();
        assert_eq!(
            parsed,
            OrderId::Tx(fixed_bytes!(
                "02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5"
            ))
        );
        let serialized = parsed.to_string();
        assert_eq!(serialized, id);
        let fixed_bytes = parsed.fixed_bytes();
        assert_eq!(
            fixed_bytes,
            fixed_bytes!("02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5")
        );

        let id = "sbundle:0x02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5";
        let parsed = OrderId::from_str(id).unwrap();
        assert_eq!(
            parsed,
            OrderId::ShareBundle(fixed_bytes!(
                "02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5"
            ))
        );
        let serialized = parsed.to_string();
        assert_eq!(serialized, id);
        let fixed_bytes = parsed.fixed_bytes();
        assert_eq!(
            fixed_bytes,
            fixed_bytes!("02e81e3cee67f25203db1178fb11070fcdace65c4eef80daa4037d9b49f011f5")
        );
    }
}
