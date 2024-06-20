use blockifier::transaction::account_transaction::AccountTransaction;
use dp_block::StarknetVersion;
use dp_convert::ToFelt;
use dp_convert::ToStarkFelt;
use dp_transactions::TxType;
use jsonrpsee::core::RpcResult;
use starknet_api::transaction::TransactionHash;
use starknet_core::types::Felt;
use starknet_core::types::TransactionTraceWithHash;
use starknet_providers::Provider;

use super::utils::tx_execution_infos_to_tx_trace;
use crate::errors::StarknetRpcApiError;
use crate::utils::execution::{block_context, re_execute_transactions};
use crate::utils::transaction::to_blockifier_transactions;
use crate::utils::{OptionExt, ResultExt};
use crate::Starknet;

// For now, we fallback to the sequencer - that is what pathfinder and juno do too, but this is temporary
pub const FALLBACK_TO_SEQUENCER_WHEN_VERSION_BELOW: StarknetVersion = StarknetVersion::STARKNET_VERSION_0_13_1_1;

pub async fn trace_transaction(starknet: &Starknet, transaction_hash: Felt) -> RpcResult<TransactionTraceWithHash> {
    let (block, tx_info) = starknet
        .block_storage()
        .find_tx_hash_block(&TransactionHash(transaction_hash.to_stark_felt()))
        .or_internal_server_error("Error while getting block from tx hash")?
        .ok_or(StarknetRpcApiError::TxnHashNotFound)?;

    let tx_index = tx_info.tx_index;

    if block.header().protocol_version < FALLBACK_TO_SEQUENCER_WHEN_VERSION_BELOW {
        // call the sequencer
        let provider = starknet.make_sequencer_provider();

        let res = provider
            .trace_transaction(transaction_hash)
            .await
            .or_internal_server_error("Error getting fallback trace response from sequencer")?;

        return Ok(TransactionTraceWithHash {
            transaction_hash: block.tx_hashes()[tx_index].to_felt(),
            trace_root: res,
        });
    }

    let block_context = block_context(starknet, block.info())?;

    // create a vector of tuples with the transaction and its hash, up to the current transaction index
    let mut transactions_before: Vec<_> = block
        .transactions()
        .iter()
        .zip(block.tx_hashes())
        .take(tx_index) // takes up until not including last tx
        .map(|(tx, hash)| to_blockifier_transactions(starknet, tx, &TransactionHash(hash.to_stark_felt())))
        .collect::<Result<_, _>>()?;

    let to_trace = transactions_before
        .pop()
        .ok_or_internal_server_error("Error: there should be at least one transaction in the block")?;

    use blockifier::transaction::transaction_execution::Transaction as BTx;
    let tx_type = match &to_trace {
        BTx::AccountTransaction(account_tx) => match account_tx {
            AccountTransaction::Declare(_) => TxType::Declare,
            AccountTransaction::DeployAccount(_) => TxType::DeployAccount,
            AccountTransaction::Invoke(_) => TxType::Invoke,
        },
        BTx::L1HandlerTransaction(_) => TxType::L1Handler,
    };

    let execution_infos = re_execute_transactions(starknet, transactions_before, [to_trace], &block_context)
        .or_internal_server_error("Failed to re-execute transactions")?
        .pop()
        .ok_or_internal_server_error("No execution info returned for the last transaction")?;

    let trace = tx_execution_infos_to_tx_trace(starknet, tx_type, &execution_infos, block.block_n())
        .or_internal_server_error("Converting execution infos to tx trace")?;

    let tx_trace = TransactionTraceWithHash { transaction_hash, trace_root: trace };

    Ok(tx_trace)
}
