//! Converts types from [`starknet_providers`] to madara's expected types.

use std::collections::HashMap;

use mp_block::DeoxysBlock;
use mp_fee::ResourcePrice;
use mp_felt::Felt252Wrapper;
use starknet_api::hash::StarkFelt;
use starknet_core::types::{
    ContractStorageDiffItem, DeclaredClassItem, DeployedContractItem, NonceUpdate, PendingStateUpdate,
    ReplacedClassItem, StateDiff as StateDiffCore, StorageEntry,
};
use starknet_ff::FieldElement;
use starknet_providers::sequencer::models::state_update::{
    DeclaredContract, DeployedContract, StateDiff as StateDiffProvider, StorageDiff as StorageDiffProvider,
};
use starknet_providers::sequencer::models::{self as p, StateUpdate as StateUpdateProvider};

use crate::commitments::lib::calculate_commitments;
use crate::utility::get_config;

pub async fn block(block: p::Block) -> DeoxysBlock {
    tokio::task::spawn_blocking(|| convert_block_sync(block)).await.expect("join error")
}

pub fn convert_block_sync(block: p::Block) -> DeoxysBlock {
    // converts starknet_provider transactions and events to mp_transactions and starknet_api events
    let transactions = transactions(block.transactions);
    let events = events(&block.transaction_receipts);

    let parent_block_hash = felt(block.parent_block_hash);
    let block_number = block.block_number.expect("no block number provided");
    let block_timestamp = block.timestamp;
    let global_state_root = felt(block.state_root.expect("no state root provided"));
    let sequencer_address = block.sequencer_address.map_or(contract_address(FieldElement::ZERO), contract_address);
    let transaction_count = transactions.len() as u128;
    let event_count = events.len() as u128;

    let (transaction_commitment, event_commitment) = commitments(&transactions, &events, block_number);

    let protocol_version = starknet_version(&block.starknet_version);
    // TODO calculate gas_price when starknet-rs supports v0.13.1
    // let l1_gas_price = resource_price(block.eth_l1_gas_price);
    let l1_gas_price = resource_price(FieldElement::ZERO);
    let extra_data = block.block_hash.map(|h| sp_core::U256::from_big_endian(&h.to_bytes_be()));

    let header = mp_block::Header {
        parent_block_hash,
        block_number,
        block_timestamp,
        global_state_root,
        sequencer_address,
        transaction_count,
        transaction_commitment,
        event_count,
        event_commitment,
        protocol_version,
        l1_gas_price,
        extra_data,
    };

    let ordered_events: Vec<mp_block::OrderedEvents> = block
        .transaction_receipts
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.events.is_empty())
        .map(|(i, r)| mp_block::OrderedEvents::new(i as u128, r.events.iter().map(event).collect()))
        .collect();

    DeoxysBlock::new(header, transactions, ordered_events)
}

fn transactions(txs: Vec<p::TransactionType>) -> Vec<mp_transactions::Transaction> {
    txs.into_iter().map(transaction).collect()
}

fn transaction(transaction: p::TransactionType) -> mp_transactions::Transaction {
    match transaction {
        p::TransactionType::InvokeFunction(tx) => mp_transactions::Transaction::Invoke(invoke_transaction(tx)),
        p::TransactionType::Declare(tx) => mp_transactions::Transaction::Declare(declare_transaction(tx)),
        p::TransactionType::Deploy(tx) => mp_transactions::Transaction::Deploy(deploy_transaction(tx)),
        p::TransactionType::DeployAccount(tx) => {
            mp_transactions::Transaction::DeployAccount(deploy_account_transaction(tx))
        }
        p::TransactionType::L1Handler(tx) => mp_transactions::Transaction::L1Handler(l1_handler_transaction(tx)),
    }
}

fn invoke_transaction(tx: p::InvokeFunctionTransaction) -> mp_transactions::InvokeTransaction {
    if tx.version == FieldElement::ZERO {
        mp_transactions::InvokeTransaction::V0(mp_transactions::InvokeTransactionV0 {
            max_fee: fee(tx.max_fee.expect("no max fee provided")),
            signature: tx.signature.into_iter().map(felt).map(Into::into).collect(),
            contract_address: felt(tx.sender_address).into(),
            entry_point_selector: felt(tx.entry_point_selector.expect("no entry_point_selector provided")).into(),
            calldata: tx.calldata.into_iter().map(felt).map(Into::into).collect(),
        })
    } else {
        mp_transactions::InvokeTransaction::V1(mp_transactions::InvokeTransactionV1 {
            max_fee: fee(tx.max_fee.expect("no max fee provided")),
            signature: tx.signature.into_iter().map(felt).map(Into::into).collect(),
            nonce: felt(tx.nonce.expect("no nonce provided")).into(),
            sender_address: felt(tx.sender_address).into(),
            calldata: tx.calldata.into_iter().map(felt).map(Into::into).collect(),
            offset_version: false,
        })
    }
}

fn declare_transaction(tx: p::DeclareTransaction) -> mp_transactions::DeclareTransaction {
    if tx.version == FieldElement::ZERO {
        mp_transactions::DeclareTransaction::V0(mp_transactions::DeclareTransactionV0 {
            max_fee: fee(tx.max_fee.expect("no max fee provided")),
            signature: tx.signature.into_iter().map(felt).map(Into::into).collect(),
            nonce: felt(tx.nonce).into(),
            class_hash: felt(tx.class_hash).into(),
            sender_address: felt(tx.sender_address).into(),
        })
    } else if tx.version == FieldElement::ONE {
        mp_transactions::DeclareTransaction::V1(mp_transactions::DeclareTransactionV1 {
            max_fee: fee(tx.max_fee.expect("no max fee provided")),
            signature: tx.signature.into_iter().map(felt).map(Into::into).collect(),
            nonce: felt(tx.nonce).into(),
            class_hash: felt(tx.class_hash).into(),
            sender_address: felt(tx.sender_address).into(),
            offset_version: false,
        })
    } else {
        mp_transactions::DeclareTransaction::V2(mp_transactions::DeclareTransactionV2 {
            max_fee: fee(tx.max_fee.expect("no max fee provided")),
            signature: tx.signature.into_iter().map(felt).map(Into::into).collect(),
            nonce: felt(tx.nonce).into(),
            class_hash: felt(tx.class_hash).into(),
            sender_address: felt(tx.sender_address).into(),
            compiled_class_hash: felt(tx.compiled_class_hash.expect("no class hash available")).into(),
            offset_version: false,
        })
    }
}

fn deploy_transaction(tx: p::DeployTransaction) -> mp_transactions::DeployTransaction {
    mp_transactions::DeployTransaction {
        version: starknet_api::transaction::TransactionVersion(felt(tx.version)),
        class_hash: felt(tx.class_hash).into(),
        contract_address: felt(tx.contract_address).into(),
        contract_address_salt: felt(tx.contract_address_salt).into(),
        constructor_calldata: tx.constructor_calldata.into_iter().map(felt).map(Into::into).collect(),
    }
}

fn deploy_account_transaction(tx: p::DeployAccountTransaction) -> mp_transactions::DeployAccountTransaction {
    mp_transactions::DeployAccountTransaction {
        max_fee: fee(tx.max_fee.expect("no max fee provided")),
        signature: tx.signature.into_iter().map(felt).map(Into::into).collect(),
        nonce: felt(tx.nonce).into(),
        contract_address_salt: felt(tx.contract_address_salt).into(),
        constructor_calldata: tx.constructor_calldata.into_iter().map(felt).map(Into::into).collect(),
        class_hash: felt(tx.class_hash).into(),
        offset_version: false,
    }
}

fn l1_handler_transaction(tx: p::L1HandlerTransaction) -> mp_transactions::HandleL1MessageTransaction {
    mp_transactions::HandleL1MessageTransaction {
        nonce: tx
            .nonce
            .ok_or("Nonce value is missing")
            .and_then(|n| u64::try_from(felt(n)).map_err(|_| "Failed to convert felt value to u64"))
            .unwrap_or_else(|e| {
                eprintln!("{}", e);
                0
            }),
        contract_address: felt(tx.contract_address).into(),
        entry_point_selector: felt(tx.entry_point_selector).into(),
        calldata: tx.calldata.into_iter().map(felt).map(Into::into).collect(),
    }
}

/// Converts a starknet version string to a felt value.
/// If the string contains more than 31 bytes, the function panics.
fn starknet_version(version: &Option<String>) -> Felt252Wrapper {
    match version {
        Some(version) => {
            Felt252Wrapper::try_from(version.as_bytes()).expect("Failed to convert version to felt: string is too long")
        }
        None => Felt252Wrapper::ZERO,
    }
}

fn fee(felt: starknet_ff::FieldElement) -> u128 {
    felt.try_into().expect("Value out of range for u128")
}

fn resource_price(eth_l1_gas_price: starknet_ff::FieldElement) -> ResourcePrice {
    ResourcePrice { price_in_strk: None, price_in_wei: fee(eth_l1_gas_price) }
}

fn events(receipts: &[p::ConfirmedTransactionReceipt]) -> Vec<starknet_api::transaction::Event> {
    receipts.iter().flat_map(|r| &r.events).map(event).collect()
}

fn event(event: &p::Event) -> starknet_api::transaction::Event {
    use starknet_api::transaction::{Event, EventContent, EventData, EventKey};

    Event {
        from_address: contract_address(event.from_address),
        content: EventContent {
            keys: event.keys.iter().copied().map(felt).map(EventKey).collect(),
            data: EventData(event.data.iter().copied().map(felt).collect()),
        },
    }
}

fn commitments(
    transactions: &[mp_transactions::Transaction],
    events: &[starknet_api::transaction::Event],
    block_number: u64,
) -> (StarkFelt, StarkFelt) {
    let chain_id = chain_id();

    let (commitment_tx, commitment_event) = calculate_commitments(transactions, events, chain_id, block_number);

    (commitment_tx.into(), commitment_event.into())
}

fn chain_id() -> mp_felt::Felt252Wrapper {
    match get_config() {
        Ok(config) => config.chain_id.into(),
        Err(e) => {
            log::error!("Failed to get chain id: {}", e);
            FieldElement::from_byte_slice_be(b"").unwrap().into()
        }
    }
}

fn felt(field_element: starknet_ff::FieldElement) -> starknet_api::hash::StarkFelt {
    starknet_api::hash::StarkFelt::new(field_element.to_bytes_be()).unwrap()
}

fn contract_address(field_element: starknet_ff::FieldElement) -> starknet_api::api_core::ContractAddress {
    starknet_api::api_core::ContractAddress(starknet_api::api_core::PatriciaKey(felt(field_element)))
}

pub fn state_update(state_update: StateUpdateProvider) -> PendingStateUpdate {
    let old_root = state_update.old_root;
    let state_diff = state_diff(state_update.state_diff);

    // StateUpdateCore { block_hash, old_root, new_root, state_diff }
    PendingStateUpdate { old_root, state_diff }
}

fn state_diff(state_diff: StateDiffProvider) -> StateDiffCore {
    let storage_diffs = storage_diffs(state_diff.storage_diffs);
    let deprecated_declared_classes = state_diff.old_declared_contracts;
    let declared_classes = declared_classes(state_diff.declared_classes);
    let deployed_contracts = deployed_contracts(state_diff.deployed_contracts);
    let replaced_classes = replaced_classes(state_diff.replaced_classes);
    let nonces = nonces(state_diff.nonces);

    StateDiffCore {
        storage_diffs,
        deprecated_declared_classes,
        declared_classes,
        deployed_contracts,
        replaced_classes,
        nonces,
    }
}

fn storage_diffs(storage_diffs: HashMap<FieldElement, Vec<StorageDiffProvider>>) -> Vec<ContractStorageDiffItem> {
    storage_diffs
        .into_iter()
        .map(|(address, entries)| ContractStorageDiffItem { address, storage_entries: storage_entries(entries) })
        .collect()
}

fn storage_entries(storage_entries: Vec<StorageDiffProvider>) -> Vec<StorageEntry> {
    storage_entries.into_iter().map(|StorageDiffProvider { key, value }| StorageEntry { key, value }).collect()
}

fn declared_classes(declared_classes: Vec<DeclaredContract>) -> Vec<DeclaredClassItem> {
    declared_classes
        .into_iter()
        .map(|DeclaredContract { class_hash, compiled_class_hash }| DeclaredClassItem {
            class_hash,
            compiled_class_hash,
        })
        .collect()
}

fn deployed_contracts(deplyed_contracts: Vec<DeployedContract>) -> Vec<DeployedContractItem> {
    deplyed_contracts
        .into_iter()
        .map(|DeployedContract { address, class_hash }| DeployedContractItem { address, class_hash })
        .collect()
}

fn replaced_classes(replaced_classes: Vec<DeployedContract>) -> Vec<ReplacedClassItem> {
    replaced_classes
        .into_iter()
        .map(|DeployedContract { address, class_hash }| ReplacedClassItem { contract_address: address, class_hash })
        .collect()
}

fn nonces(nonces: HashMap<FieldElement, FieldElement>) -> Vec<NonceUpdate> {
    // TODO: make sure the order is `contract_address` -> `nonce`
    // and not `nonce` -> `contract_address`
    nonces.into_iter().map(|(contract_address, nonce)| NonceUpdate { contract_address, nonce }).collect()
}
