use std::sync::Arc;

use crossbeam_skiplist::SkipMap;
use rocksdb::{WriteBatchWithTransaction, WriteOptions};
use starknet_types_core::felt::Felt;

use super::{DeoxysStorageError, StorageType, StorageView, StorageViewMut};
use crate::{Column, DatabaseExt, DB};

pub struct ContractClassHashesView(Arc<DB>);
#[derive(Debug)]
pub struct ContractClassHashesViewMut(Arc<DB>, SkipMap<Felt, Felt>);

impl ContractClassHashesView {
    pub(crate) fn new(backend: Arc<DB>) -> Self {
        Self(backend)
    }
}
impl ContractClassHashesViewMut {
    pub(crate) fn new(backend: Arc<DB>) -> Self {
        Self(backend, Default::default())
    }
}

impl StorageView for ContractClassHashesView {
    type KEY = Felt;
    type VALUE = Felt;

    fn get(&self, class_hash: &Self::KEY) -> Result<Option<Self::VALUE>, super::DeoxysStorageError> {
        let db = &self.0;
        let column = db.get_column(Column::ContractClassHashes);

        let compiled_class_hash = db
            .get_cf(&column, bincode::serialize(&class_hash)?)
            .map_err(|_| DeoxysStorageError::StorageRetrievalError(StorageType::ContractClassHashes))?
            .map(|bytes| bincode::deserialize::<Felt>(&bytes[..]));

        match compiled_class_hash {
            Some(Ok(compiled_class_hash)) => Ok(Some(compiled_class_hash)),
            Some(Err(_)) => Err(DeoxysStorageError::StorageDecodeError(StorageType::ContractClassHashes)),
            None => Ok(None),
        }
    }

    fn contains(&self, class_hash: &Self::KEY) -> Result<bool, super::DeoxysStorageError> {
        let db = &self.0;
        let column = db.get_column(Column::ContractClassHashes);

        match db.key_may_exist_cf(&column, bincode::serialize(&class_hash)?) {
            true => Ok(self.get(class_hash)?.is_some()),
            false => Ok(false),
        }
    }
}

impl StorageViewMut for ContractClassHashesViewMut {
    type KEY = Felt;
    type VALUE = Felt;

    fn insert(&self, class_hash: Self::KEY, compiled_class_hash: Self::VALUE) -> Result<(), DeoxysStorageError> {
        self.1.insert(class_hash, compiled_class_hash);
        Ok(())
    }

    fn commit(self, _block_number: u64) -> Result<(), DeoxysStorageError> {
        let db = &self.0;
        let column = db.get_column(Column::ContractClassHashes);

        let mut batch = WriteBatchWithTransaction::<true>::default();
        for (key, value) in self.1.into_iter() {
            batch.put_cf(&column, bincode::serialize(&key)?, bincode::serialize(&value)?);
        }
        let mut write_opt = WriteOptions::default();
        write_opt.disable_wal(true);
        db.write_opt(batch, &write_opt)
            .map_err(|_| DeoxysStorageError::StorageCommitError(StorageType::ContractClassHashes))
    }
}
