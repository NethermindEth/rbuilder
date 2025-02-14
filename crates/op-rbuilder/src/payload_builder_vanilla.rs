use crate::generator::BuildArguments;
use crate::{
    generator::{BlockCell, PayloadBuilder},
    metrics::OpRBuilderMetrics,
    tx_signer::Signer,
};
use alloy_consensus::{
    Eip658Value, Header, Transaction, TxEip1559, Typed2718, EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::merge::BEACON_NONCE;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_engine::PayloadId;
use op_alloy_consensus::{OpDepositReceipt, OpTxType, OpTypedTransaction};
use alloy_rpc_types_eth::Withdrawals;
use reth::core::primitives::InMemorySize;
use reth_basic_payload_builder::*;
use reth_chain_state::{ExecutedBlock, ExecutedBlockWithTrieUpdates};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_evm::{env::EvmEnv, system_calls::SystemCaller, ConfigureEvm, ConfigureEvmEnv, ConfigureEvmFor, Evm, NextBlockEnvAttributes};
use reth_execution_types::ExecutionOutcome;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_consensus::calculate_receipt_root_no_memo_optimism;
use reth_optimism_forks::OpHardforks;
use reth_optimism_payload_builder::{error::OpPayloadBuilderError, payload::{OpBuiltPayload, OpPayloadBuilderAttributes}, OpPayloadPrimitives};
use reth_optimism_primitives::{OpPrimitives, OpReceipt, OpTransactionSigned, ADDRESS_L2_TO_L1_MESSAGE_PASSER};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_payload_util::PayloadTransactions;
use reth_primitives::{
    transaction::SignedTransactionIntoRecoveredExt, Block, BlockBody, RecoveredBlock, SealedHeader,
};
use reth_primitives_traits::{proofs, Block as _, NodePrimitives};
use reth_provider::{HashedPostStateProvider, ProviderError, StateProviderFactory, StateRootProvider, StorageRootProvider};
use reth_revm::database::StateProviderDatabase;
use reth_payload_util::BestPayloadTransactions;
use reth_transaction_pool::{BestTransactionsAttributes, PoolTransaction, TransactionPool};
use revm::{
    db::{states::bundle_state::BundleRetention, State},
    primitives::{EVMError, InvalidTransaction, ResultAndState, SpecId},
    Database, DatabaseCommit,
};
use tokio_util::sync::CancellationToken;
use std::{fmt::Display, sync::Arc, time::Instant};
use alloy_consensus::constants::EMPTY_WITHDRAWALS;
use reth_optimism_evm::{OpReceiptBuilder, ReceiptBuilderCtx};
use reth_optimism_payload_builder::config::{OpBuilderConfig, OpDAConfig};
use reth_optimism_primitives::transaction::signed::OpTransaction;
use revm::primitives::ExecutionResult;
use tracing::{info, trace, warn};

/// Optimism's payload builder
#[derive(Debug, Clone)]
pub struct OpPayloadBuilderVanilla<Pool, Client, EvmConfig, N: NodePrimitives, Txs = ()> {
    /// The type responsible for creating the evm.
    pub evm_config: EvmConfig,
    /// Transaction pool.
    pub pool: Pool,
    /// Node client.
    pub client: Client,
    /// Settings for the builder, e.g. DA settings.
    pub config: OpBuilderConfig,
    /// The builder's signer key to use for an end of block tx
    pub builder_signer: Option<Signer>,
    /// The type responsible for yielding the best transactions for the payload if mempool
    /// transactions are allowed.
    pub best_transactions: Txs,
    /// Node primitive types.
    pub receipt_builder: Arc<dyn OpReceiptBuilder<N::SignedTx, Receipt = N::Receipt>>,
    /// The metrics for the builder
    pub metrics: OpRBuilderMetrics,
}

impl<Pool, Client, EvmConfig, N: NodePrimitives> OpPayloadBuilderVanilla<Pool, Client, EvmConfig, N> {
    /// `OpPayloadBuilder` constructor.
    pub fn new(
        pool: Pool,
        client: Client,
        evm_config: EvmConfig,
        builder_signer: Option<Signer>,
        receipt_builder: impl OpReceiptBuilder<N::SignedTx, Receipt = N::Receipt>,
    ) -> Self {
        Self::with_builder_config(pool, client, evm_config, receipt_builder, builder_signer, Default::default())
    }

    pub fn with_builder_config(
        pool: Pool,
        client: Client,
        evm_config: EvmConfig,
        builder_signer: Option<Signer>,
        receipt_builder: impl OpReceiptBuilder<N::SignedTx, Receipt = N::Receipt>,
        config: OpBuilderConfig,
    ) -> Self {
        Self {
            pool,
            client,
            receipt_builder: Arc::new(receipt_builder),
            config,
            evm_config,
            best_transactions: (),
            metrics: Default::default(),
            builder_signer,
        }
    }
}

impl<Pool, Client, EvmConfig, N, Txs> PayloadBuilder
for OpPayloadBuilderVanilla<Pool, Client, EvmConfig, N, Txs>
where
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthChainSpec + OpHardforks> + Clone,
    N: OpPayloadPrimitives,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = N::SignedTx>>,
    EvmConfig: ConfigureEvmFor<N>,
    Txs: OpPayloadTransactions<Pool::Transaction>,
{
    type Attributes = OpPayloadBuilderAttributes<N::SignedTx>;
    type BuiltPayload = OpBuiltPayload<N>;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let pool = self.pool.clone();
        let block_build_start_time = Instant::now();

        let payload = self.build_payload(args, |attrs| self.best_transactions.best_transactions(pool, attrs))?;
        match payload {
            BuildOutcome::Better(..) => {
                self.metrics
                    .total_block_built_duration
                    .record(block_build_start_time.elapsed());
                self.metrics.block_built_success.increment(1);
            },
            BuildOutcome::Freeze(..) => {
                self.metrics
                    .total_block_built_duration
                    .record(block_build_start_time.elapsed());
            },
            BuildOutcome::Cancelled => {
                tracing::warn!("Payload build cancelled");
                return Err(PayloadBuilderError::MissingPayload)
            }
            _ => {
                tracing::warn!("No better payload found");
                return Err(PayloadBuilderError::MissingPayload)
            }
        }
        Ok(payload)
    }
}



impl<Pool, Client, EvmConfig, N, T> OpPayloadBuilderVanilla<Pool, Client, EvmConfig, N, T>
where
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = N::SignedTx>>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthChainSpec + OpHardforks>,
    N: OpPayloadPrimitives,
    EvmConfig: ConfigureEvmFor<N>,
{
    /// Constructs an Optimism payload from the transactions sent via the
    /// Payload attributes by the sequencer. If the `no_tx_pool` argument is passed in
    /// the payload attributes, the transaction pool will be ignored and the only transactions
    /// included in the payload will be those sent through the attributes.
    ///
    /// Given build arguments including an Optimism client, transaction pool,
    /// and configuration, this function creates a transaction payload. Returns
    /// a result indicating success with the payload or an error in case of failure.
    fn build_payload<'a, Txs>(
        &self,
        args: BuildArguments<OpPayloadBuilderAttributes<N::SignedTx>, OpBuiltPayload<N>>,
        best: impl FnOnce(BestTransactionsAttributes) -> Txs + Send + Sync + 'a,
    ) -> Result<BuildOutcome<OpBuiltPayload<N>>, PayloadBuilderError>
    where
        Txs: PayloadTransactions<Transaction: PoolTransaction<Consensus = N::SignedTx>>,
    {
        let evm_env = self
            .eth_evm(&args.config.attributes, &args.config.parent_header)
            .map_err(PayloadBuilderError::other)?;

        let BuildArguments {
            mut cached_reads,
            config,
            cancel,
            best_payload
        } = args;

        let ctx = OpPayloadBuilderCtx {
            evm_config: self.evm_config.clone(),
            chain_spec: self.client.chain_spec(),
            config,
            evm_env,
            cancel,
            builder_signer: self.builder_signer,
            metrics: Default::default(),
            best_payload,
            receipt_builder: self.receipt_builder.clone(),
        };

        let builder = OpBuilder::new(best);

        let state_provider = self.client.state_by_block_hash(ctx.parent().hash())?;
        let state = StateProviderDatabase::new(state_provider);

        let payload = if ctx.attributes().no_tx_pool {
            let db = State::builder()
                .with_database(state)
                .with_bundle_update()
                .build();
            builder.build(db, ctx)
        } else {
            // sequencer mode we can reuse cachedreads from previous runs
            let db = State::builder()
                .with_database(cached_reads.as_db_mut(state))
                .with_bundle_update()
                .build();
            builder.build(db, ctx)
        }
            .map(|out| out.with_cached_reads(cached_reads));

    }

    /// Returns the configured [`EvmEnv`] for the targeted payload
    /// (that has the `parent` as its parent).
    pub fn evm_env(
        &self,
        attributes: &OpPayloadBuilderAttributes<N::SignedTx>,
        parent: &Header,
    ) -> Result<EvmEnv<EvmConfig::Spec>, EvmConfig::Error> {
        let next_attributes = NextBlockEnvAttributes {
            timestamp: attributes.timestamp(),
            suggested_fee_recipient: attributes.suggested_fee_recipient(),
            prev_randao: attributes.prev_randao(),
            gas_limit: attributes.gas_limit.unwrap_or(parent.gas_limit),
        };
        self.evm_config.next_evm_env(parent, next_attributes)
    }

}

/// The type that builds the payload.
///
/// Payload building for optimism is composed of several steps.
/// The first steps are mandatory and defined by the protocol.
///
/// 1. first all System calls are applied.
/// 2. After canyon the forced deployed `create2deployer` must be loaded
/// 3. all sequencer transactions are executed (part of the payload attributes)
///
/// Depending on whether the node acts as a sequencer and is allowed to include additional
/// transactions (`no_tx_pool == false`):
/// 4. include additional transactions
///
/// And finally
/// 5. build the block: compute all roots (txs, state)
#[derive(derive_more::Debug)]
pub struct OpBuilder<'a, Txs> {
    /// Yields the best transaction to include if transactions from the mempool are allowed.
    best: Box<dyn FnOnce(BestTransactionsAttributes) -> Txs + 'a>,
}

impl<'a, Txs> OpBuilder<'a, Txs> {
    fn new(best: impl FnOnce(BestTransactionsAttributes) -> Txs + Send + Sync + 'a) -> Self {
        Self {
            best: Box::new(best),
        }
    }
}

impl<Txs> OpBuilder<'_, Txs> {
    /// Executes the payload and returns the outcome.
    pub fn execute<EvmConfig, ChainSpec, N, DB, P>(
        self,
        state: &mut State<DB>,
        ctx: &OpPayloadBuilderCtx<EvmConfig, ChainSpec, N>,
    ) -> Result<BuildOutcomeKind<ExecutedPayload<N>>, PayloadBuilderError>
    where
        N: OpPayloadPrimitives,
        Txs: PayloadTransactions<Transaction: PoolTransaction<Consensus = N::SignedTx>>,
        EvmConfig: ConfigureEvmFor<N>,
        ChainSpec: EthChainSpec + OpHardforks,
        DB: Database<Error = ProviderError> + AsRef<P>,
        P: StorageRootProvider,
    {
        let Self { best } = self;
        info!(target: "payload_builder", id=%ctx.payload_id(), parent_header = ?ctx.parent().hash(), parent_number = ctx.parent().number, "building new payload");

        // 1. apply eip-4788 pre block contract call
        ctx.apply_pre_beacon_root_contract_call(state)?;

        // 2. ensure create2deployer is force deployed
        ctx.ensure_create2_deployer(state)?;

        let sequencer_tx_start_time = Instant::now();

        // 3. execute sequencer transactions
        let mut info = ctx.execute_sequencer_transactions(state)?;

        ctx.metrics
            .sequencer_tx_duration
            .record(sequencer_tx_start_time.elapsed());

        // 4. if mem pool transactions are requested we execute them

        // gas reserved for builder tx
        let message = format!("Block Number: {}", ctx.block_number())
            .as_bytes()
            .to_vec();
        let builder_tx_gas = ctx.builder_signer().map_or(0, |_| {
            OpPayloadBuilderCtx::<EvmConfig, ChainSpec, N>::estimate_gas_for_builder_tx(message.clone())
        });
        let block_gas_limit = ctx.block_gas_limit() - builder_tx_gas;
        if !ctx.attributes().no_tx_pool {
            let best_txs_start_time = Instant::now();
            let best_txs = best(ctx.best_transaction_attributes());
            ctx.metrics
                .transaction_pool_fetch_duration
                .record(best_txs_start_time.elapsed());
            if ctx
                .execute_best_transactions(&mut info, state, best_txs, block_gas_limit)?
                .is_some()
            {
                return Ok(BuildOutcomeKind::Cancelled);
            }
        }

        // Add builder tx to the block
        ctx.add_builder_tx(&mut info, state, builder_tx_gas, message);

        let state_merge_start_time = Instant::now();

        // merge all transitions into bundle state, this would apply the withdrawal balance changes
        // and 4788 contract call
        state.merge_transitions(BundleRetention::Reverts);

        ctx.metrics
            .state_transition_merge_duration
            .record(state_merge_start_time.elapsed());
        ctx.metrics
            .payload_num_tx
            .record(info.executed_transactions.len() as f64);

        let withdrawals_root = if ctx.is_isthmus_active() {
            // withdrawals root field in block header is used for storage root of L2 predeploy
            // `l2tol1-message-passer`
            Some(
                state
                    .database
                    .as_ref()
                    .storage_root(ADDRESS_L2_TO_L1_MESSAGE_PASSER, Default::default())?,
            )
        } else if ctx.is_canyon_active() {
            Some(EMPTY_WITHDRAWALS)
        } else {
            None
        };

        let payload = ExecutedPayload { info, withdrawals_root };

        Ok(BuildOutcomeKind::Better { payload })

    }

    /// Builds the payload on top of the state.
    pub fn build<EvmConfig, ChainSpec, N, DB, P>(
        self,
        mut state: State<DB>,
        ctx: OpPayloadBuilderCtx<EvmConfig, ChainSpec, N>,
    ) -> Result<BuildOutcomeKind<OpBuiltPayload<N>>, PayloadBuilderError>
    where
        EvmConfig: ConfigureEvmFor<N>,
        ChainSpec: EthChainSpec + OpHardforks,
        N: OpPayloadPrimitives,
        Txs: PayloadTransactions<Transaction: PoolTransaction<Consensus = N::SignedTx>>,
        DB: Database<Error = ProviderError> + AsRef<P>,
        P: StateRootProvider + HashedPostStateProvider + StorageRootProvider,
    {
        let ExecutedPayload {
            info,
            withdrawals_root,
        } = match self.execute(&mut state, &ctx)? {
            BuildOutcomeKind::Better { payload } | BuildOutcomeKind::Freeze(payload) => payload,
            BuildOutcomeKind::Cancelled => return Ok(BuildOutcomeKind::Cancelled),
            BuildOutcomeKind::Aborted { fees } => return Ok(BuildOutcomeKind::Aborted { fees }),
        };

        let block_number = ctx.block_number();
        let execution_outcome = ExecutionOutcome::new(
            state.take_bundle(),
            vec![info.receipts],
            block_number,
            Vec::new(),
        );
        let receipts_root = execution_outcome
            .generic_receipts_root_slow(block_number, |receipts| {
                calculate_receipt_root_no_memo_optimism(
                    receipts,
                    &ctx.chain_spec,
                    ctx.attributes().timestamp(),
                )
            })
            .expect("Number is in range");
        let logs_bloom = execution_outcome
            .block_logs_bloom(block_number)
            .expect("Number is in range");

        // calculate the state root
        let state_root_start_time = Instant::now();

        let state_provider = state.database.as_ref();
        let hashed_state = state_provider.hashed_post_state(execution_outcome.state());
        let (state_root, trie_output) = {
            state
                .database
                .as_ref()
                .state_root_with_updates(hashed_state.clone())
                .inspect_err(|err| {
                    warn!(target: "payload_builder",
                    parent_header=%ctx.parent().hash(),
                        %err,
                        "failed to calculate state root for payload"
                    );
                })?
        };

        ctx.metrics
            .state_root_calculation_duration
            .record(state_root_start_time.elapsed());

        // create the block header
        let transactions_root = proofs::calculate_transaction_root(&info.executed_transactions);

        // OP doesn't support blobs/EIP-4844.
        // https://specs.optimism.io/protocol/exec-engine.html#ecotone-disable-blob-transactions
        // Need [Some] or [None] based on hardfork to match block hash.
        let (excess_blob_gas, blob_gas_used) = ctx.blob_fields();
        let extra_data = ctx.extra_data()?;

        let header = Header {
            parent_hash: ctx.parent().hash(),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: ctx.evm_env.block_env.coinbase,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root,
            logs_bloom,
            timestamp: ctx.attributes().payload_attributes.timestamp,
            mix_hash: ctx.attributes().payload_attributes.prev_randao,
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(ctx.base_fee()),
            number: ctx.parent().number + 1,
            gas_limit: ctx.block_gas_limit(),
            difficulty: U256::ZERO,
            gas_used: info.cumulative_gas_used,
            extra_data,
            parent_beacon_block_root: ctx.attributes().payload_attributes.parent_beacon_block_root,
            blob_gas_used,
            excess_blob_gas,
            requests_hash: None,
        };

        // seal the block
        let block = N::Block::new(
            header,
            BlockBody {
                transactions: info.executed_transactions,
                ommers: vec![],
                withdrawals: ctx.withdrawals().cloned(),
            },
        );

        let sealed_block = Arc::new(block.seal_slow());
        info!(target: "payload_builder", id=%ctx.attributes().payload_id(), sealed_block_header = ?sealed_block.header(), "sealed built block");

        // create the executed block data
        let executed: ExecutedBlockWithTrieUpdates<OpPrimitives> = ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(RecoveredBlock::new_sealed(
                    sealed_block.as_ref().clone(),
                    info.executed_senders,
                )),
                execution_output: Arc::new(execution_outcome),
                hashed_state: Arc::new(hashed_state),
            },
            trie: Arc::new(trie_output),
        };

        let no_tx_pool = ctx.attributes().no_tx_pool;

        let payload =
            OpBuiltPayload::new(ctx.payload_id(), sealed_block, info.total_fees, Some(executed));

        ctx.metrics
            .payload_byte_size
            .record(payload.block().size() as f64);

        if no_tx_pool {
            // if `no_tx_pool` is set only transactions from the payload attributes will be included
            // in the payload. In other words, the payload is deterministic and we can
            // freeze it once we've successfully built it.
            Ok(BuildOutcomeKind::Freeze(payload))
        } else {
            Ok(BuildOutcomeKind::Better { payload })
        }
    }
}

/// A type that returns a the [`PayloadTransactions`] that should be included in the pool.
pub trait OpPayloadTransactions<Transaction>: Clone + Send + Sync + Unpin + 'static {
    /// Returns an iterator that yields the transaction in the order they should get included in the
    /// new payload.
    fn best_transactions<Pool: TransactionPool<Transaction = Transaction>>(
        &self,
        pool: Pool,
        attr: BestTransactionsAttributes,
    ) -> impl PayloadTransactions<Transaction = Transaction>;
}

impl<T: PoolTransaction> OpPayloadTransactions<T> for () {
    fn best_transactions<Pool: TransactionPool<Transaction = T>>(
        &self,
        pool: Pool,
        attr: BestTransactionsAttributes,
    ) -> impl PayloadTransactions<Transaction = T> {
        BestPayloadTransactions::new(pool.best_transactions_with_attributes(attr))
    }
}

/// Holds the state after execution
#[derive(Debug)]
pub struct ExecutedPayload<N: NodePrimitives> {
    /// Tracked execution info
    pub info: ExecutionInfo<N>,
    /// Withdrawal hash.
    pub withdrawals_root: Option<B256>,
}

/// This acts as the container for executed transactions and its byproducts (receipts, gas used)
#[derive(Default, Debug)]
pub struct ExecutionInfo<N: NodePrimitives> {
    /// All executed transactions (unrecovered).
    pub executed_transactions: Vec<OpTransactionSigned>,
    /// The recovered senders for the executed transactions.
    pub executed_senders: Vec<Address>,
    /// The transaction receipts
    pub receipts: Vec<N::Receipt>,
    /// All gas used so far
    pub cumulative_gas_used: u64,
    /// Estimated DA size
    pub cumulative_da_bytes_used: u64,
    /// Tracks fees from executed mempool transactions
    pub total_fees: U256,
}

impl<N: NodePrimitives> ExecutionInfo<N> {
    /// Create a new instance with allocated slots.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            executed_transactions: Vec::with_capacity(capacity),
            executed_senders: Vec::with_capacity(capacity),
            receipts: Vec::with_capacity(capacity),
            cumulative_gas_used: 0,
            cumulative_da_bytes_used: 0,
            total_fees: U256::ZERO,
        }
    }

    /// Returns true if the transaction would exceed the block limits:
    /// - block gas limit: ensures the transaction still fits into the block.
    /// - tx DA limit: if configured, ensures the tx does not exceed the maximum allowed DA limit
    ///   per tx.
    /// - block DA limit: if configured, ensures the transaction's DA size does not exceed the
    ///   maximum allowed DA limit per block.
    pub fn is_tx_over_limits(
        &self,
        tx: &N::SignedTx,
        block_gas_limit: u64,
        tx_data_limit: Option<u64>,
        block_data_limit: Option<u64>,
    ) -> bool {
        if tx_data_limit.is_some_and(|da_limit| tx.length() as u64 > da_limit) {
            return true;
        }

        if block_data_limit
            .is_some_and(|da_limit| self.cumulative_da_bytes_used + (tx.length() as u64) > da_limit)
        {
            return true;
        }

        self.cumulative_gas_used + tx.gas_limit() > block_gas_limit
    }
}

/// Container type that holds all necessities to build a new payload.
#[derive(Debug)]
pub struct OpPayloadBuilderCtx<EvmConfig: ConfigureEvmEnv, ChainSpec, N: NodePrimitives> {
    /// The type that knows how to perform system calls and configure the evm.
    pub evm_config: EvmConfig,
    /// The DA config for the payload builder
    pub da_config: OpDAConfig,
    /// The chainspec
    pub chain_spec: Arc<OpChainSpec>,
    /// How to build the payload.
    pub config: PayloadConfig<OpPayloadBuilderAttributes<N::SignedTx>>,
    /// EVM environment.
    pub evm_env: EvmEnv,
    /// Marker to check whether the job has been cancelled.
    pub cancel: CancellationToken,
    /// The builder signer
    pub builder_signer: Option<Signer>,
    /// The metrics for the builder
    pub metrics: OpRBuilderMetrics,
    /// The currently best payload.
    pub best_payload: Option<OpBuiltPayload>,
    /// Receipt builder.
    pub receipt_builder: Arc<dyn OpReceiptBuilder<N::SignedTx, Receipt = N::Receipt>>,
}

impl<EvmConfig, ChainSpec, N> OpPayloadBuilderCtx<EvmConfig, ChainSpec, N>
where
    EvmConfig: ConfigureEvmEnv,
    ChainSpec: EthChainSpec + OpHardforks,
    N: NodePrimitives,
{
    /// Returns the parent block the payload will be build on.
    pub fn parent(&self) -> &SealedHeader {
        &self.config.parent_header
    }

    /// Returns the builder attributes.
    pub const fn attributes(&self) -> &OpPayloadBuilderAttributes<N::SignedTx> {
        &self.config.attributes
    }

    /// Returns the withdrawals if shanghai is active.
    pub fn withdrawals(&self) -> Option<&Withdrawals> {
        self.chain_spec
            .is_shanghai_active_at_timestamp(self.attributes().timestamp())
            .then(|| &self.attributes().payload_attributes.withdrawals)
    }

    /// Returns the block gas limit to target.
    pub fn block_gas_limit(&self) -> u64 {
        self.attributes()
            .gas_limit
            .unwrap_or_else(|| self.evm_env.block_env.gas_limit.saturating_to())
    }

    /// Returns the block number for the block.
    pub fn block_number(&self) -> u64 {
        self.evm_env.block_env.number.to()
    }

    /// Returns the current base fee
    pub fn base_fee(&self) -> u64 {
        self.evm_env.block_env.basefee.to()
    }

    /// Returns the current blob gas price.
    pub fn get_blob_gasprice(&self) -> Option<u64> {
        self.evm_env
            .block_env
            .get_blob_gasprice()
            .map(|gasprice| gasprice as u64)
    }

    /// Returns the blob fields for the header.
    ///
    /// This will always return `Some(0)` after ecotone.
    pub fn blob_fields(&self) -> (Option<u64>, Option<u64>) {
        // OP doesn't support blobs/EIP-4844.
        // https://specs.optimism.io/protocol/exec-engine.html#ecotone-disable-blob-transactions
        // Need [Some] or [None] based on hardfork to match block hash.
        if self.is_ecotone_active() {
            (Some(0), Some(0))
        } else {
            (None, None)
        }
    }

    /// Returns the extra data for the block.
    ///
    /// After holocene this extracts the extradata from the paylpad
    pub fn extra_data(&self) -> Result<Bytes, PayloadBuilderError> {
        if self.is_holocene_active() {
            self.attributes()
                .get_holocene_extra_data(
                    self.chain_spec.base_fee_params_at_timestamp(
                        self.attributes().payload_attributes.timestamp,
                    ),
                )
                .map_err(PayloadBuilderError::other)
        } else {
            Ok(Default::default())
        }
    }

    /// Returns the current fee settings for transactions from the mempool
    pub fn best_transaction_attributes(&self) -> BestTransactionsAttributes {
        BestTransactionsAttributes::new(self.base_fee(), self.get_blob_gasprice())
    }

    /// Returns the unique id for this payload job.
    pub fn payload_id(&self) -> PayloadId {
        self.attributes().payload_id()
    }

    /// Returns true if regolith is active for the payload.
    pub fn is_regolith_active(&self) -> bool {
        self.chain_spec
            .is_regolith_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if ecotone is active for the payload.
    pub fn is_ecotone_active(&self) -> bool {
        self.chain_spec
            .is_ecotone_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if canyon is active for the payload.
    pub fn is_canyon_active(&self) -> bool {
        self.chain_spec
            .is_canyon_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if holocene is active for the payload.
    pub fn is_holocene_active(&self) -> bool {
        self.chain_spec
            .is_holocene_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns true if isthmus is active for the payload.
    pub fn is_isthmus_active(&self) -> bool {
        self.chain_spec.is_isthmus_active_at_timestamp(self.attributes().timestamp())
    }

    /// Returns the chain id
    pub fn chain_id(&self) -> u64 {
        self.chain_spec.chain.id()
    }

    /// Returns the builder signer
    pub fn builder_signer(&self) -> Option<Signer> {
        self.builder_signer
    }

    /// Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
    /// blocks will always have at least a single transaction in them (the L1 info transaction),
    /// so we can safely assume that this will always be triggered upon the transition and that
    /// the above check for empty blocks will never be hit on OP chains.
    pub fn ensure_create2_deployer<DB>(&self, db: &mut State<DB>) -> Result<(), PayloadBuilderError>
    where
        DB: Database,
        DB::Error: Display,
    {
        reth_optimism_evm::ensure_create2_deployer(
            self.chain_spec.clone(),
            self.attributes().payload_attributes.timestamp,
            db,
        )
        .map_err(|err| {
            warn!(target: "payload_builder", %err, "missing create2 deployer, skipping block.");
            PayloadBuilderError::other(OpPayloadBuilderError::ForceCreate2DeployerFail)
        })
    }
}

impl<EvmConfig, ChainSpec, N> OpPayloadBuilderCtx<EvmConfig, ChainSpec, N>
where
    EvmConfig: ConfigureEvmFor<N>,
    ChainSpec: EthChainSpec + OpHardforks,
    N: OpPayloadPrimitives,
{
    /// apply eip-4788 pre block contract call
    pub fn apply_pre_beacon_root_contract_call<DB>(
        &self,
        db: &mut DB,
    ) -> Result<(), PayloadBuilderError>
    where
        DB: Database<Error = ProviderError> + DatabaseCommit,
    {
        SystemCaller::new(self.evm_config.clone(), self.chain_spec.clone())
            .pre_block_beacon_root_contract_call(
                db,
                &self.evm_env,
                self.attributes()
                    .payload_attributes
                    .parent_beacon_block_root,
            )
            .map_err(|err| {
                warn!(target: "payload_builder",
                    parent_header=%self.parent().hash(),
                    %err,
                    "failed to apply beacon root contract call for payload"
                );
                PayloadBuilderError::Internal(err.into())
            })?;

        Ok(())
    }

    /// Constructs a receipt for the given transaction.
    fn build_receipt(
        &self,
        info: &ExecutionInfo<N>,
        result: ExecutionResult,
        deposit_nonce: Option<u64>,
        tx: &N::SignedTx,
    ) -> N::Receipt {
        match self.receipt_builder.build_receipt(ReceiptBuilderCtx {
            tx,
            result,
            cumulative_gas_used: info.cumulative_gas_used,
        }) {
            Ok(receipt) => receipt,
            Err(ctx) => {
                let receipt = alloy_consensus::Receipt {
                    // Success flag was added in `EIP-658: Embedding transaction status code
                    // in receipts`.
                    status: Eip658Value::Eip658(ctx.result.is_success()),
                    cumulative_gas_used: ctx.cumulative_gas_used,
                    logs: ctx.result.into_logs(),
                };

                self.receipt_builder.build_deposit_receipt(OpDepositReceipt {
                    inner: receipt,
                    deposit_nonce,
                    // The deposit receipt version was introduced in Canyon to indicate an
                    // update to how receipt hashes should be computed
                    // when set. The state transition process ensures
                    // this is only set for post-Canyon deposit
                    // transactions.
                    deposit_receipt_version: self.is_canyon_active().then_some(1),
                })
            }
        }
    }

    /// Executes all sequencer transactions that are included in the payload attributes.
    pub fn execute_sequencer_transactions<DB>(
        &self,
        db: &mut State<DB>,
    ) -> Result<ExecutionInfo<N>, PayloadBuilderError>
    where
        DB: Database<Error = ProviderError>,
    {
        let mut info = ExecutionInfo::with_capacity(self.attributes().transactions.len());

        let mut evm = self.evm_config.evm_with_env(&mut *db, self.evm_env.clone());
        for sequencer_tx in &self.attributes().transactions {
            // A sequencer's block should never contain blob transactions.
            if sequencer_tx.value().is_eip4844() {
                return Err(PayloadBuilderError::other(
                    OpPayloadBuilderError::BlobTransactionRejected,
                ));
            }

            // Convert the transaction to a [RecoveredTx]. This is
            // purely for the purposes of utilizing the `evm_config.tx_env`` function.
            // Deposit transactions do not have signatures, so if the tx is a deposit, this
            // will just pull in its `from` address.
            let sequencer_tx = sequencer_tx
                .value()
                .try_clone_into_recovered()
                .map_err(|_| {
                    PayloadBuilderError::other(OpPayloadBuilderError::TransactionEcRecoverFailed)
                })?;

            // Cache the depositor account prior to the state transition for the deposit nonce.
            //
            // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
            // were not introduced in Bedrock. In addition, regular transactions don't have deposit
            // nonces, so we don't need to touch the DB for those.
            let depositor_nonce = (self.is_regolith_active() && sequencer_tx.is_deposit())
                .then(|| {
                    evm.db_mut()
                        .load_cache_account(sequencer_tx.signer())
                        .map(|acc| acc.account_info().unwrap_or_default().nonce)
                })
                .transpose()
                .map_err(|_| {
                    PayloadBuilderError::other(OpPayloadBuilderError::AccountLoadFailed(
                        sequencer_tx.signer(),
                    ))
                })?;

            let tx_env = self
                .evm_config
                .tx_env(sequencer_tx.tx(), sequencer_tx.signer());

            let ResultAndState { result, state } = match evm.transact(tx_env) {
                Ok(res) => res,
                Err(err) => {
                    if err.is_invalid_tx_err() {
                        trace!(target: "payload_builder", %err, ?sequencer_tx, "Error in sequencer transaction, skipping.");
                        continue
                    }
                    // this is an error that we should treat as fatal for this attempt
                    return Err(PayloadBuilderError::EvmExecutionError(Box::new(err)))
                }
            };

            // commit changes
            evm.db_mut().commit(state);

            let gas_used = result.gas_used();

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            info.cumulative_gas_used += gas_used;

            info.receipts.push(self.build_receipt(
                &info,
                result,
                depositor_nonce,
                sequencer_tx.tx(),
            ));

            // append sender and transaction to the respective lists
            info.executed_senders.push(sequencer_tx.signer());
            info.executed_transactions.push(sequencer_tx.into_tx());
        }

        Ok(info)
    }

    /// Executes the given best transactions and updates the execution info.
    ///
    /// Returns `Ok(Some(())` if the job was cancelled.
    pub fn execute_best_transactions<DB>(
        &self,
        info: &mut ExecutionInfo<N>,
        db: &mut State<DB>,
        mut best_txs: impl PayloadTransactions<
            Transaction: PoolTransaction<Consensus = EvmConfig::Transaction>,
        >,
        // We pass custom block_gas_limit because we need to leave some space for builder tx
        block_gas_limit: u64,
    ) -> Result<Option<()>, PayloadBuilderError>
    where
        DB: Database<Error = ProviderError>,
    {
        let execute_txs_start_time = Instant::now();
        let mut num_txs_considered = 0;
        let mut num_txs_simulated = 0;
        let mut num_txs_simulated_success = 0;
        let mut num_txs_simulated_fail = 0;
        let base_fee = self.base_fee();

        let mut evm = self.evm_config.evm_with_env(&mut *db, self.evm_env.clone());
        while let Some(tx) = best_txs.next(()) {
            let tx = tx.into_consensus();
            num_txs_considered += 1;
            // ensure we still have capacity for this transaction
            /// TODO: include DA checks in here
            if info.cumulative_gas_used + tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as
                // invalid which also removes all dependent transaction from
                // the iterator before we can continue
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue;
            }

            // A sequencer's block should never contain blob or deposit transactions from the pool.
            if tx.is_eip4844() || tx.is_deposit() {
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue;
            }

            // check if the job was cancelled, if so we can exit early
            if self.cancel.is_cancelled() {
                return Ok(Some(()));
            }

            // Configure the environment for the tx.
            let tx_env = self.evm_config.tx_env(tx.tx(), tx.signer());

            let tx_simulation_start_time = Instant::now();

            let ResultAndState { result, state } = match evm.transact(tx_env) {
                Ok(res) => res,
                Err(err) => {
                    if let Some(err) = err.as_invalid_tx_err() {
                        if err.is_nonce_too_low() {
                            // if the nonce is too low, we can skip this transaction
                            trace!(target: "payload_builder", %err, ?tx, "skipping nonce too low transaction");
                        } else {
                            // if the transaction is invalid, we can skip it and all of its
                            // descendants
                            trace!(target: "payload_builder", %err, ?tx, "skipping invalid transaction and its descendants");
                            best_txs.mark_invalid(tx.signer(), tx.nonce());
                        }

                        continue;
                    }
                    // this is an error that we should treat as fatal for this attempt
                    return Err(PayloadBuilderError::EvmExecutionError(Box::new(err)));
                }
            };

            self.metrics
                .tx_simulation_duration
                .record(tx_simulation_start_time.elapsed());
            self.metrics.tx_byte_size.record(tx.tx().size() as f64);
            num_txs_simulated += 1;
            if result.is_success() {
                num_txs_simulated_success += 1;
            } else {
                num_txs_simulated_fail += 1;
            }
            self.metrics
                .payload_num_tx_simulated
                .record(num_txs_simulated as f64);
            self.metrics
                .payload_num_tx_simulated_success
                .record(num_txs_simulated_success as f64);
            self.metrics
                .payload_num_tx_simulated_fail
                .record(num_txs_simulated_fail as f64);

            // commit changes
            evm.db_mut().commit(state);

            let gas_used = result.gas_used();

            // add gas used by the transaction to cumulative gas used, before creating the
            // receipt
            info.cumulative_gas_used += gas_used;

            info.receipts.push(self.build_receipt(info, result, None, &tx));

            // update add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(base_fee)
                .expect("fee is always valid; execution succeeded");
            info.total_fees += U256::from(miner_fee) * U256::from(gas_used);

            // append sender and transaction to the respective lists
            info.executed_senders.push(tx.signer());
            info.executed_transactions.push(tx.into_tx());
        }

        self.metrics
            .payload_tx_simulation_duration
            .record(execute_txs_start_time.elapsed());
        self.metrics
            .payload_num_tx_considered
            .record(num_txs_considered as f64);

        Ok(None)
    }

    pub fn add_builder_tx<DB>(
        &self,
        info: &mut ExecutionInfo<N>,
        db: &mut State<DB>,
        builder_tx_gas: u64,
        message: Vec<u8>,
    ) -> Option<()>
    where
        DB: Database<Error = ProviderError>,
    {
        self.builder_signer()
            .map(|signer| {
                let base_fee = self.base_fee();
                // Create message with block number for the builder to sign
                let nonce = db
                    .load_cache_account(signer.address)
                    .map(|acc| acc.account_info().unwrap_or_default().nonce)
                    .map_err(|_| {
                        PayloadBuilderError::other(OpPayloadBuilderError::AccountLoadFailed(
                            signer.address,
                        ))
                    })?;

                // Create the EIP-1559 transaction
                let eip1559 = OpTypedTransaction::Eip1559(TxEip1559 {
                    chain_id: self.chain_id(),
                    nonce,
                    gas_limit: builder_tx_gas,
                    max_fee_per_gas: base_fee.into(),
                    max_priority_fee_per_gas: 0,
                    to: TxKind::Call(Address::ZERO),
                    // Include the message as part of the transaction data
                    input: message.into(),
                    ..Default::default()
                });
                let tx = eip1559;

                // Sign the transaction
                let builder_tx = signer.sign_tx(tx).map_err(PayloadBuilderError::other)?;

                let mut evm = self.evm_config.evm_with_env(&mut *db, self.evm_env.clone());
                let tx_env = self.evm_config.tx_env(builder_tx.tx(), builder_tx.signer());
                let ResultAndState { result, state } = evm
                    .transact(tx_env)
                    .map_err(|err| PayloadBuilderError::EvmExecutionError(Box::new(err)))?;

                // Release the db reference by dropping evm
                drop(evm);
                // Commit changes
                db.commit(state);

                let gas_used = result.gas_used();

                // Add gas used by the transaction to cumulative gas used, before creating the receipt
                info.cumulative_gas_used += gas_used;

                // Push transaction changeset and calculate header bloom filter for receipt
                info.receipts.push(self.build_receipt(info, result, None, &tx));

                // Append sender and transaction to the respective lists
                info.executed_senders.push(builder_tx.signer());
                info.executed_transactions.push(builder_tx.into_tx());
                Ok(())
            })
            .transpose()
            .unwrap_or_else(|err: PayloadBuilderError| {
                warn!(target: "payload_builder", %err, "Failed to add builder transaction");
                None
            })
    }

    fn estimate_gas_for_builder_tx(input: Vec<u8>) -> u64 {
        // Count zero and non-zero bytes
        let (zero_bytes, nonzero_bytes) = input.iter().fold((0, 0), |(zeros, nonzeros), &byte| {
            if byte == 0 {
                (zeros + 1, nonzeros)
            } else {
                (zeros, nonzeros + 1)
            }
        });

        // Calculate gas cost (4 gas per zero byte, 16 gas per non-zero byte)
        let zero_cost = zero_bytes * 4;
        let nonzero_cost = nonzero_bytes * 16;

        zero_cost + nonzero_cost + 21_000
    }
}
