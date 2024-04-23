use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use blockifier::execution::contract_class::ContractClass as BlockifierContractClass;
use cairo_lang_starknet_classes::casm_contract_class::{
    CasmContractClass, CasmContractEntryPoint, CasmContractEntryPoints,
};
use mc_sync::l1::ETHEREUM_STATE_UPDATE;
use mp_block::DeoxysBlock;
use mp_felt::Felt252Wrapper;
use mp_hashers::HasherT;
use mp_transactions::to_starknet_core_transaction::to_starknet_core_tx;
use mp_types::block::{DBlockT, DHashT};
use num_bigint::BigUint;
use pallet_starknet_runtime_api::{ConvertTransactionRuntimeApi, StarknetRuntimeApi};
use sc_client_api::backend::{Backend, StorageProvider};
use sc_client_api::BlockBackend;
use sc_transaction_pool::ChainApi;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_runtime::DispatchError;
use starknet_api::deprecated_contract_class::{EntryPoint, EntryPointType};
use starknet_api::hash::StarkFelt;
use starknet_api::state::ThinStateDiff;
use starknet_api::transaction as stx;
use starknet_core::types::contract::{CompiledClass, CompiledClassEntrypoint, CompiledClassEntrypointList};
use starknet_core::types::{
    BlockId, BlockStatus, CompressedLegacyContractClass, ContractClass, ContractStorageDiffItem, DeclaredClassItem,
    DeployedContractItem, EntryPointsByType, FieldElement, FlattenedSierraClass, FromByteArrayError,
    LegacyContractEntryPoint, LegacyEntryPointsByType, NonceUpdate, ReplacedClassItem, StateDiff, StorageEntry,
};

use crate::errors::StarknetRpcApiError;
use crate::madara_backend_client::get_block_by_block_hash;
use crate::{Felt, Starknet};

pub(crate) fn tx_hash_retrieve(tx_hashes: Vec<StarkFelt>) -> Vec<FieldElement> {
    // safe to unwrap because we know that the StarkFelt is a valid FieldElement
    tx_hashes.iter().map(|tx_hash| FieldElement::from_bytes_be(&tx_hash.0).unwrap()).collect()
}

pub(crate) fn tx_hash_compute<H>(block: &DeoxysBlock, chain_id: Felt) -> Vec<FieldElement>
where
    H: HasherT + Send + Sync + 'static,
{
    // safe to unwrap because we know that the StarkFelt is a valid FieldElement
    block
        .transactions_hashes::<H>(chain_id.0.into(), Some(block.header().block_number))
        .map(|tx_hash| FieldElement::from_bytes_be(&tx_hash.0.0).unwrap())
        .collect()
}

pub(crate) fn tx_conv(
    txs: &[stx::Transaction],
    tx_hashes: Vec<FieldElement>,
) -> Vec<starknet_core::types::Transaction> {
    txs.iter().zip(tx_hashes).map(|(tx, hash)| to_starknet_core_tx(tx.clone(), hash)).collect()
}

pub(crate) fn status(block_number: u64) -> BlockStatus {
    if block_number <= ETHEREUM_STATE_UPDATE.read().unwrap().block_number {
        BlockStatus::AcceptedOnL1
    } else {
        BlockStatus::AcceptedOnL2
    }
}

pub fn previous_substrate_block_hash<A, BE, G, C, P, H>(
    starknet: &Starknet<A, BE, G, C, P, H>,
    substrate_block_hash: DHashT,
) -> Result<DHashT, StarknetRpcApiError>
where
    A: ChainApi<Block = DBlockT> + 'static,
    C: HeaderBackend<DBlockT> + BlockBackend<DBlockT> + StorageProvider<DBlockT, BE> + 'static,
    C: ProvideRuntimeApi<DBlockT>,
    C::Api: StarknetRuntimeApi<DBlockT> + ConvertTransactionRuntimeApi<DBlockT>,
    H: HasherT + Send + Sync + 'static,
    BE: Backend<DBlockT> + 'static,
{
    let starknet_block = get_block_by_block_hash(starknet.client.as_ref(), substrate_block_hash).map_err(|e| {
        log::error!("Failed to get block for block hash {substrate_block_hash}: '{e}'");
        StarknetRpcApiError::InternalServerError
    })?;
    let block_number = starknet_block.header().block_number;
    let previous_block_number = match block_number {
        0 => 0,
        _ => block_number - 1,
    };
    let substrate_block_hash =
        starknet.substrate_block_hash_from_starknet_block(BlockId::Number(previous_block_number)).map_err(|e| {
            log::error!("Failed to retrieve previous block substrate hash: {e}");
            StarknetRpcApiError::InternalServerError
        })?;

    Ok(substrate_block_hash)
}

/// Returns a [`ContractClass`] from a [`BlockifierContractClass`]
#[allow(dead_code)]
pub(crate) fn to_rpc_contract_class(contract_class: BlockifierContractClass) -> Result<ContractClass> {
    match contract_class {
        BlockifierContractClass::V0(contract_class) => {
            let entry_points_by_type: HashMap<_, _> = contract_class.entry_points_by_type.clone().into_iter().collect();
            let entry_points_by_type = to_legacy_entry_points_by_type(&entry_points_by_type)?;
            let compressed_program = compress(&contract_class.program.serialize()?)?;
            Ok(ContractClass::Legacy(CompressedLegacyContractClass {
                program: compressed_program,
                entry_points_by_type,
                // FIXME 723
                abi: None,
            }))
        }
        BlockifierContractClass::V1(_contract_class) => Ok(ContractClass::Sierra(FlattenedSierraClass {
            sierra_program: vec![], // FIXME: https://github.com/keep-starknet-strange/madara/issues/775
            contract_class_version: option_env!("COMPILER_VERSION").unwrap_or("0.11.2").into(),
            entry_points_by_type: EntryPointsByType { constructor: vec![], external: vec![], l1_handler: vec![] }, /* TODO: add entry_points_by_type */
            abi: String::from("{}"), // FIXME: https://github.com/keep-starknet-strange/madara/issues/790
        })),
    }
}

/// Returns a [`StateDiff`] from a [`ThinStateDiff`]
pub(crate) fn to_rpc_state_diff(thin_state_diff: ThinStateDiff) -> StateDiff {
    let nonces = thin_state_diff
        .nonces
        .iter()
        .map(|x| NonceUpdate {
            contract_address: Felt252Wrapper::from(x.0.0.0).into(),
            nonce: Felt252Wrapper::from(x.1.0).into(),
        })
        .collect();

    let storage_diffs = thin_state_diff
        .storage_diffs
        .iter()
        .map(|x| ContractStorageDiffItem {
            address: Felt252Wrapper::from(x.0.0.0).into(),
            storage_entries: x
                .1
                .iter()
                .map(|y| StorageEntry {
                    key: Felt252Wrapper::from(y.0.0.0).into(),
                    value: Felt252Wrapper::from(*y.1).into(),
                })
                .collect(),
        })
        .collect();

    let deprecated_declared_classes =
        thin_state_diff.deprecated_declared_classes.iter().map(|x| Felt252Wrapper::from(x.0).into()).collect();

    let declared_classes = thin_state_diff
        .declared_classes
        .iter()
        .map(|x| DeclaredClassItem {
            class_hash: Felt252Wrapper::from(x.0.0).into(),
            compiled_class_hash: Felt252Wrapper::from(x.1.0).into(),
        })
        .collect();

    let deployed_contracts = thin_state_diff
        .deployed_contracts
        .iter()
        .map(|x| DeployedContractItem {
            address: Felt252Wrapper::from(x.0.0.0).into(),
            class_hash: Felt252Wrapper::from(x.1.0).into(),
        })
        .collect();

    let replaced_classes = thin_state_diff
        .replaced_classes
        .iter()
        .map(|x| ReplacedClassItem {
            contract_address: Felt252Wrapper::from(x.0.0.0).into(),
            class_hash: Felt252Wrapper::from(x.1.0).into(),
        })
        .collect();

    StateDiff {
        nonces,
        storage_diffs,
        deprecated_declared_classes,
        declared_classes,
        deployed_contracts,
        replaced_classes,
    }
}

/// Returns a compressed vector of bytes
fn compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut gzip_encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    // 2023-08-22: JSON serialization is already done in Blockifier
    // https://github.com/keep-starknet-strange/blockifier/blob/no_std-support-7578442/crates/blockifier/src/execution/contract_class.rs#L129
    // https://github.com/keep-starknet-strange/blockifier/blob/no_std-support-7578442/crates/blockifier/src/execution/contract_class.rs#L389
    // serde_json::to_writer(&mut gzip_encoder, data)?;
    gzip_encoder.write_all(data)?;
    Ok(gzip_encoder.finish()?)
}

/// Returns a [Result<LegacyEntryPointsByType>] (starknet-rs type) from a [HashMap<EntryPointType,
/// Vec<EntryPoint>>]
fn to_legacy_entry_points_by_type(
    entries: &HashMap<EntryPointType, Vec<EntryPoint>>,
) -> Result<LegacyEntryPointsByType> {
    fn collect_entry_points(
        entries: &HashMap<EntryPointType, Vec<EntryPoint>>,
        entry_point_type: EntryPointType,
    ) -> Result<Vec<LegacyContractEntryPoint>> {
        Ok(entries
            .get(&entry_point_type)
            .ok_or(anyhow!("Missing {:?} entry point", entry_point_type))?
            .iter()
            .map(|e| to_legacy_entry_point(e.clone()))
            .collect::<Result<Vec<LegacyContractEntryPoint>, FromByteArrayError>>()?)
    }

    let constructor = collect_entry_points(entries, EntryPointType::Constructor)?;
    let external = collect_entry_points(entries, EntryPointType::External)?;
    let l1_handler = collect_entry_points(entries, EntryPointType::L1Handler)?;

    Ok(LegacyEntryPointsByType { constructor, external, l1_handler })
}

/// Returns a [LegacyContractEntryPoint] (starknet-rs) from a [EntryPoint] (starknet-api)
fn to_legacy_entry_point(entry_point: EntryPoint) -> Result<LegacyContractEntryPoint, FromByteArrayError> {
    let selector = FieldElement::from_bytes_be(&entry_point.selector.0.0)?;
    let offset = entry_point.offset.0;
    Ok(LegacyContractEntryPoint { selector, offset })
}

// Utils to convert Casm contract class to Compiled class
#[allow(dead_code)]
pub fn get_casm_contract_class_hash(casm_contract_class: &CasmContractClass) -> FieldElement {
    let compiled_class = casm_contract_class_to_compiled_class(casm_contract_class);
    compiled_class.class_hash().unwrap()
}

/// Converts a [CasmContractClass] to a [CompiledClass]
fn casm_contract_class_to_compiled_class(casm_contract_class: &CasmContractClass) -> CompiledClass {
    CompiledClass {
        prime: casm_contract_class.prime.to_string(),
        compiler_version: casm_contract_class.compiler_version.clone(),
        bytecode: casm_contract_class.bytecode.iter().map(|x| biguint_to_field_element(&x.value)).collect(),
        entry_points_by_type: casm_entry_points_to_compiled_entry_points(&casm_contract_class.entry_points_by_type),
        hints: vec![],                    // not needed to get class hash so ignoring this
        pythonic_hints: None,             // not needed to get class hash so ignoring this
        bytecode_segment_lengths: vec![], // TODO: implement this
    }
}

/// Converts a [CasmContractEntryPoints] to a [CompiledClassEntrypointList]
fn casm_entry_points_to_compiled_entry_points(value: &CasmContractEntryPoints) -> CompiledClassEntrypointList {
    CompiledClassEntrypointList {
        external: value.external.iter().map(casm_entry_point_to_compiled_entry_point).collect(),
        l1_handler: value.l1_handler.iter().map(casm_entry_point_to_compiled_entry_point).collect(),
        constructor: value.constructor.iter().map(casm_entry_point_to_compiled_entry_point).collect(),
    }
}

/// Converts a [CasmContractEntryPoint] to a [CompiledClassEntrypoint]
fn casm_entry_point_to_compiled_entry_point(value: &CasmContractEntryPoint) -> CompiledClassEntrypoint {
    CompiledClassEntrypoint {
        selector: biguint_to_field_element(&value.selector),
        offset: value.offset.try_into().unwrap(),
        builtins: value.builtins.clone(),
    }
}

/// Converts a [BigUint] to a [FieldElement]
fn biguint_to_field_element(value: &BigUint) -> FieldElement {
    let bytes = value.to_bytes_be();
    FieldElement::from_byte_slice_be(bytes.as_slice()).unwrap()
}

pub fn convert_error<C, T>(
    client: Arc<C>,
    best_block_hash: DHashT,
    call_result: Result<T, DispatchError>,
) -> Result<T, StarknetRpcApiError>
where
    C: ProvideRuntimeApi<DBlockT>,
    C::Api: StarknetRuntimeApi<DBlockT> + ConvertTransactionRuntimeApi<DBlockT>,
{
    match call_result {
        Ok(val) => Ok(val),
        Err(e) => match client.runtime_api().convert_error(best_block_hash, e) {
            Ok(starknet_error) => Err(starknet_error.into()),
            Err(_) => Err(StarknetRpcApiError::InternalServerError),
        },
    }
}