use std::borrow::Cow;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::SystemTime;

use blockifier::blockifier::stateful_validator::StatefulValidatorError;
use blockifier::transaction::account_transaction::AccountTransaction;
use blockifier::transaction::transactions::DeclareTransaction;
use blockifier::transaction::transactions::DeployAccountTransaction;
use blockifier::transaction::transactions::InvokeTransaction;
use dc_db::db_block_id::DbBlockId;
use dc_db::storage_handler::DeoxysStorageError;
use dc_db::DeoxysBackend;
use dc_exec::ExecutionContext;
use dp_block::header::PendingHeader;
use dp_block::{
    BlockId, BlockTag, DeoxysBlockInner, DeoxysMaybePendingBlock, DeoxysMaybePendingBlockInfo, DeoxysPendingBlockInfo,
};
use inner::MempoolInner;
use starknet_api::core::{ContractAddress, Nonce};

pub mod block_production;
mod inner;
mod l1;

pub use inner::{ArrivedAtTimestamp, MempoolTransaction};
pub use l1::L1DataProvider;
use starknet_api::transaction::TransactionHash;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Storage error: {0:#}")]
    StorageError(#[from] DeoxysStorageError),
    #[error("No genesis block in storage")]
    NoGenesis,
    #[error("Internal error: {0}")]
    Internal(Cow<'static, str>),
    #[error("Validation error: {0:#}")]
    Validation(#[from] StatefulValidatorError),
    #[error(transparent)]
    InnerMempool(#[from] inner::TxInsersionError),
    #[error(transparent)]
    Exec(#[from] dc_exec::Error),
}

pub struct Mempool {
    backend: Arc<DeoxysBackend>,
    l1_data_provider: Arc<dyn L1DataProvider>,
    inner: RwLock<MempoolInner>,
}

impl Mempool {
    pub fn new(backend: Arc<DeoxysBackend>, l1_data_provider: Arc<dyn L1DataProvider>) -> Self {
        Mempool { backend, l1_data_provider, inner: Default::default() }
    }

    /// This function creates the pending block if it is not found.
    // TODO: move this somewhere else
    pub(crate) fn get_or_create_pending_block(&self) -> Result<DeoxysMaybePendingBlock, Error> {
        match self.backend.get_block(&DbBlockId::Pending)? {
            Some(block) => Ok(block),
            None => {
                // No pending block: we create one :)

                let block_info =
                    self.backend.get_block_info(&BlockId::Tag(BlockTag::Latest))?.ok_or(Error::NoGenesis)?;
                let block_info = block_info.as_nonpending().ok_or(Error::Internal("Latest block is pending".into()))?;

                Ok(DeoxysMaybePendingBlock {
                    info: DeoxysMaybePendingBlockInfo::Pending(DeoxysPendingBlockInfo {
                        header: PendingHeader {
                            parent_block_hash: block_info.block_hash,
                            sequencer_address: **self.backend.chain_config().sequencer_address,
                            block_timestamp: SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .expect("Current time is before the unix timestamp")
                                .as_secs(),
                            protocol_version: self.backend.chain_config().latest_protocol_version,
                            l1_gas_price: self.l1_data_provider.get_gas_prices(),
                            l1_da_mode: self.l1_data_provider.get_da_mode(),
                        },
                        tx_hashes: vec![],
                    }),
                    inner: DeoxysBlockInner { transactions: vec![], receipts: vec![] },
                })
            }
        }
    }

    pub fn accept_account_tx(&self, tx: AccountTransaction) -> Result<(), Error> {
        // The timestamp *does not* take the transaction validation time into account.
        let arrived_at = ArrivedAtTimestamp::now();

        // Get pending block
        let pending_block_info = self.get_or_create_pending_block()?;

        // If the contract has been deployed for the same block is is invoked, we need to skip validations.
        // NB: the lock is NOT taken the entire time the tx is being validated. As such, the deploy tx
        //  may appear during that time - but it is not a problem.
        let deploy_account_tx_hash = if let AccountTransaction::Invoke(tx) = &tx {
            let mempool = self.inner.read().expect("Poisoned lock");
            if mempool.has_deployed_contract(&tx.tx.sender_address()) {
                Some(tx.tx_hash) // we return the wrong tx hash here but it's ok because the actual hash is unused by blockifier
            } else {
                None
            }
        } else {
            None
        };

        // Perform validations
        let exec_context = ExecutionContext::new(&self.backend, &pending_block_info.info)?;
        let mut validator = exec_context.tx_validator();
        validator.perform_validations(clone_account_tx(&tx), deploy_account_tx_hash)?;

        if !is_only_query(&tx) {
            // Finally, add it to the nonce chain for the account nonce
            let force = false;
            self.inner.write().expect("Poisoned lock").insert_tx(MempoolTransaction { tx, arrived_at }, force)?
        }

        Ok(())
    }

    pub fn take_txs_chunk(&self, dest: &mut Vec<MempoolTransaction>, n: usize) {
        let mut inner = self.inner.write().expect("Poisoned lock");
        inner.pop_next_chunk(dest, n)
    }

    pub fn take_tx(&self) -> Option<MempoolTransaction> {
        let mut inner = self.inner.write().expect("Poisoned lock");
        inner.pop_next()
    }

    pub fn readd_txs(&self, txs: Vec<MempoolTransaction>) {
        let mut inner = self.inner.write().expect("Poisoned lock");
        inner.readd_txs(txs)
    }
}

pub(crate) fn is_only_query(tx: &AccountTransaction) -> bool {
    match tx {
        AccountTransaction::Declare(tx) => tx.only_query(),
        AccountTransaction::DeployAccount(tx) => tx.only_query,
        AccountTransaction::Invoke(tx) => tx.only_query,
    }
}

pub(crate) fn contract_addr(tx: &AccountTransaction) -> ContractAddress {
    match tx {
        AccountTransaction::Declare(tx) => tx.tx.sender_address(),
        AccountTransaction::DeployAccount(tx) => tx.contract_address,
        AccountTransaction::Invoke(tx) => tx.tx.sender_address(),
    }
}

pub(crate) fn nonce(tx: &AccountTransaction) -> Nonce {
    match tx {
        AccountTransaction::Declare(tx) => tx.tx.nonce(),
        AccountTransaction::DeployAccount(tx) => tx.tx.nonce(),
        AccountTransaction::Invoke(tx) => tx.tx.nonce(),
    }
}

pub(crate) fn tx_hash(tx: &AccountTransaction) -> TransactionHash {
    match tx {
        AccountTransaction::Declare(tx) => tx.tx_hash,
        AccountTransaction::DeployAccount(tx) => tx.tx_hash,
        AccountTransaction::Invoke(tx) => tx.tx_hash,
    }
}

// AccountTransaction does not implement Clone :(
pub(crate) fn clone_account_tx(tx: &AccountTransaction) -> AccountTransaction {
    match tx {
        // Declare has a private field :(
        AccountTransaction::Declare(tx) => AccountTransaction::Declare(match tx.only_query() {
            true => DeclareTransaction::new_for_query(tx.tx.clone(), tx.tx_hash, tx.class_info.clone()).unwrap(),
            false => DeclareTransaction::new(tx.tx.clone(), tx.tx_hash, tx.class_info.clone()).unwrap(),
        }),
        AccountTransaction::DeployAccount(tx) => AccountTransaction::DeployAccount(DeployAccountTransaction {
            tx: tx.tx.clone(),
            tx_hash: tx.tx_hash,
            contract_address: tx.contract_address,
            only_query: tx.only_query,
        }),
        AccountTransaction::Invoke(tx) => AccountTransaction::Invoke(InvokeTransaction {
            tx: tx.tx.clone(),
            tx_hash: tx.tx_hash,
            only_query: tx.only_query,
        }),
    }
}