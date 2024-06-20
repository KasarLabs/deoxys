use dp_convert::ToFelt;
use starknet_core::types::{Felt, Hash256};

use crate::{
    DataAvailabilityResources, DeclareTransactionReceipt, DeployAccountTransactionReceipt, DeployTransactionReceipt,
    Event, ExecutionResources, ExecutionResult, FeePayment, InvokeTransactionReceipt, L1HandlerTransactionReceipt,
    MsgToL1, PriceUnit, TransactionReceipt,
};

impl TransactionReceipt {
    pub fn from_provider(
        receipt: starknet_providers::sequencer::models::ConfirmedTransactionReceipt,
        tx_type: &starknet_providers::sequencer::models::TransactionType,
    ) -> Self {
        match tx_type {
            starknet_providers::sequencer::models::TransactionType::Declare(_) => {
                TransactionReceipt::Declare(DeclareTransactionReceipt::from(receipt))
            }
            starknet_providers::sequencer::models::TransactionType::Deploy(tx) => {
                TransactionReceipt::Deploy(DeployTransactionReceipt::from_provider(receipt, tx.contract_address))
            }
            starknet_providers::sequencer::models::TransactionType::DeployAccount(tx) => {
                TransactionReceipt::DeployAccount(DeployAccountTransactionReceipt::from_provider(
                    receipt,
                    tx.contract_address.unwrap_or_default(),
                ))
            }
            starknet_providers::sequencer::models::TransactionType::InvokeFunction(_) => {
                TransactionReceipt::Invoke(InvokeTransactionReceipt::from(receipt))
            }
            starknet_providers::sequencer::models::TransactionType::L1Handler(_tx) => {
                // TODO compute message hash
                TransactionReceipt::L1Handler(L1HandlerTransactionReceipt::from_provider(
                    receipt,
                    Hash256::from_hex("0x0").unwrap(),
                ))
            }
        }
    }
}

impl From<starknet_providers::sequencer::models::ConfirmedTransactionReceipt> for DeclareTransactionReceipt {
    fn from(receipt: starknet_providers::sequencer::models::ConfirmedTransactionReceipt) -> Self {
        Self {
            transaction_hash: receipt.transaction_hash,
            actual_fee: receipt.actual_fee.into(),
            messages_sent: receipt.l2_to_l1_messages.into_iter().map(MsgToL1::from).collect(),
            events: receipt.events.into_iter().map(Event::from).collect(),
            execution_resources: receipt.execution_resources.map(ExecutionResources::from).unwrap_or_default(),
            execution_result: execution_result(receipt.execution_status, receipt.revert_error),
        }
    }
}

impl DeployTransactionReceipt {
    fn from_provider(
        receipt: starknet_providers::sequencer::models::ConfirmedTransactionReceipt,
        contract_address: Felt,
    ) -> Self {
        Self {
            transaction_hash: receipt.transaction_hash,
            actual_fee: receipt.actual_fee.into(),
            messages_sent: receipt.l2_to_l1_messages.into_iter().map(MsgToL1::from).collect(),
            events: receipt.events.into_iter().map(Event::from).collect(),
            execution_resources: receipt.execution_resources.map(ExecutionResources::from).unwrap_or_default(),
            execution_result: execution_result(receipt.execution_status, receipt.revert_error),
            contract_address,
        }
    }
}

impl DeployAccountTransactionReceipt {
    fn from_provider(
        receipt: starknet_providers::sequencer::models::ConfirmedTransactionReceipt,
        contract_address: Felt,
    ) -> Self {
        Self {
            transaction_hash: receipt.transaction_hash,
            actual_fee: receipt.actual_fee.into(),
            messages_sent: receipt.l2_to_l1_messages.into_iter().map(MsgToL1::from).collect(),
            events: receipt.events.into_iter().map(Event::from).collect(),
            execution_resources: receipt.execution_resources.map(ExecutionResources::from).unwrap_or_default(),
            execution_result: execution_result(receipt.execution_status, receipt.revert_error),
            contract_address,
        }
    }
}

impl From<starknet_providers::sequencer::models::ConfirmedTransactionReceipt> for InvokeTransactionReceipt {
    fn from(receipt: starknet_providers::sequencer::models::ConfirmedTransactionReceipt) -> Self {
        Self {
            transaction_hash: receipt.transaction_hash,
            actual_fee: receipt.actual_fee.into(),
            messages_sent: receipt.l2_to_l1_messages.into_iter().map(MsgToL1::from).collect(),
            events: receipt.events.into_iter().map(Event::from).collect(),
            execution_resources: receipt.execution_resources.map(ExecutionResources::from).unwrap_or_default(),
            execution_result: execution_result(receipt.execution_status, receipt.revert_error),
        }
    }
}

impl L1HandlerTransactionReceipt {
    fn from_provider(
        receipt: starknet_providers::sequencer::models::ConfirmedTransactionReceipt,
        message_hash: Hash256,
    ) -> Self {
        Self {
            message_hash,
            transaction_hash: receipt.transaction_hash,
            actual_fee: receipt.actual_fee.into(),
            messages_sent: receipt.l2_to_l1_messages.into_iter().map(MsgToL1::from).collect(),
            events: receipt.events.into_iter().map(Event::from).collect(),
            execution_resources: receipt.execution_resources.map(ExecutionResources::from).unwrap_or_default(),
            execution_result: execution_result(receipt.execution_status, receipt.revert_error),
        }
    }
}

impl From<Felt> for FeePayment {
    fn from(fee: Felt) -> Self {
        Self { amount: fee, unit: PriceUnit::Wei }
    }
}

impl From<starknet_providers::sequencer::models::L2ToL1Message> for MsgToL1 {
    fn from(msg: starknet_providers::sequencer::models::L2ToL1Message) -> Self {
        Self { from_address: msg.from_address, to_address: msg.to_address.to_felt(), payload: msg.payload }
    }
}

impl From<starknet_providers::sequencer::models::Event> for Event {
    fn from(event: starknet_providers::sequencer::models::Event) -> Self {
        Self { from_address: event.from_address, keys: event.keys, data: event.data }
    }
}

impl From<starknet_providers::sequencer::models::ExecutionResources> for ExecutionResources {
    fn from(resources: starknet_providers::sequencer::models::ExecutionResources) -> Self {
        let builtin_instance_counter = resources.builtin_instance_counter;
        Self {
            steps: resources.n_steps,
            memory_holes: Some(resources.n_memory_holes),
            range_check_builtin_applications: builtin_instance_counter.range_check_builtin,
            pedersen_builtin_applications: builtin_instance_counter.pedersen_builtin,
            poseidon_builtin_applications: builtin_instance_counter.poseidon_builtin,
            ec_op_builtin_applications: builtin_instance_counter.ec_op_builtin,
            ecdsa_builtin_applications: builtin_instance_counter.ecdsa_builtin,
            bitwise_builtin_applications: builtin_instance_counter.bitwise_builtin,
            keccak_builtin_applications: builtin_instance_counter.keccak_builtin,
            segment_arena_builtin: builtin_instance_counter.segment_arena_builtin,
            data_availability: resources.data_availability.map(DataAvailabilityResources::from).unwrap_or_default(),
        }
    }
}

fn execution_result(
    status: Option<starknet_providers::sequencer::models::TransactionExecutionStatus>,
    revert_error: Option<String>,
) -> ExecutionResult {
    let reason = revert_error.unwrap_or_default();
    match status {
        Some(starknet_providers::sequencer::models::TransactionExecutionStatus::Succeeded) => {
            ExecutionResult::Succeeded
        }
        Some(starknet_providers::sequencer::models::TransactionExecutionStatus::Reverted) => {
            ExecutionResult::Reverted { reason }
        }
        Some(starknet_providers::sequencer::models::TransactionExecutionStatus::Rejected) => {
            ExecutionResult::Reverted { reason }
        }
        None => ExecutionResult::Reverted { reason },
    }
}
