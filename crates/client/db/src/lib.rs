//! Deoxys database

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fmt, fs};

use anyhow::{Context, Result};
use bonsai_db::{BonsaiDb, DatabaseKeyMapping};
use bonsai_trie::id::BasicId;
use bonsai_trie::{BonsaiStorage, BonsaiStorageConfig};
use mapping_db::MappingDb;
use rocksdb::backup::{BackupEngine, BackupEngineOptions};

mod error;
pub mod mapping_db;
use rocksdb::{
    BoundColumnFamily, ColumnFamilyDescriptor, DBCompressionType, Env, FlushOptions, MultiThreaded,
    OptimisticTransactionDB, Options, SliceTransform,
};
use starknet_types_core::hash::{Pedersen, Poseidon, StarkHash};
pub mod bonsai_db;
pub mod storage_handler;
pub mod storage_updates;

pub use error::{BonsaiDbError, DbError};
use storage_handler::block_state_diff::BlockStateDiffView;
use storage_handler::class_trie::{ClassTrieView, ClassTrieViewMut};
use storage_handler::compiled_contract_class::{CompiledContractClassView, CompiledContractClassViewMut};
use storage_handler::contract_class_data::{ContractClassDataView, ContractClassDataViewMut};
use storage_handler::contract_class_hashes::{ContractClassHashesView, ContractClassHashesViewMut};
use storage_handler::contract_data::{
    ContractClassView, ContractClassViewMut, ContractNoncesView, ContractNoncesViewMut,
};
use storage_handler::contract_storage::{ContractStorageView, ContractStorageViewMut};
use storage_handler::contract_storage_trie::{ContractStorageTrieView, ContractStorageTrieViewMut};
use storage_handler::contract_trie::{ContractTrieView, ContractTrieViewMut};
use tokio::sync::{mpsc, oneshot};

const DB_HASH_LEN: usize = 32;
/// Hash type that this backend uses for the database.
pub type DbHash = [u8; DB_HASH_LEN];

pub type DB = OptimisticTransactionDB<MultiThreaded>;

pub use rocksdb;
pub type WriteBatchWithTransaction = rocksdb::WriteBatchWithTransaction<true>;

pub(crate) async fn open_rocksdb(
    path: &Path,
    create: bool,
    backup_dir: Option<PathBuf>,
    restore_from_latest_backup: bool,
) -> Result<(Arc<OptimisticTransactionDB<MultiThreaded>>, Option<mpsc::Sender<BackupRequest>>)> {
    let mut opts = Options::default();
    opts.set_report_bg_io_stats(true);
    opts.set_use_fsync(false);
    opts.create_if_missing(create);
    opts.create_missing_column_families(true);
    opts.set_bytes_per_sync(1024 * 1024);
    opts.set_keep_log_file_num(1);
    opts.optimize_level_style_compaction(4096 * 1024 * 1024);
    opts.set_compression_type(DBCompressionType::Zstd);
    let cores = std::thread::available_parallelism().map(|e| e.get() as i32).unwrap_or(1);
    opts.increase_parallelism(cores);

    opts.set_atomic_flush(true);
    opts.set_manual_wal_flush(true);
    opts.set_max_subcompactions(cores as _);

    let mut env = Env::new().context("creating rocksdb env")?;
    // env.set_high_priority_background_threads(cores); // flushes
    env.set_low_priority_background_threads(cores); // compaction

    opts.set_env(&env);

    let backup_hande = if let Some(backup_dir) = backup_dir {
        let (restored_cb_sender, restored_cb_recv) = oneshot::channel();

        let (sender, receiver) = mpsc::channel(1);
        let db_path = path.to_owned();
        std::thread::spawn(move || {
            spawn_backup_db_task(&backup_dir, restore_from_latest_backup, &db_path, restored_cb_sender, receiver)
                .expect("database backup thread")
        });

        log::debug!("blocking on db restoration");
        restored_cb_recv.await.context("restoring database")?;
        log::debug!("done blocking on db restoration");

        Some(sender)
    } else {
        None
    };

    log::debug!("opening db at {:?}", path.display());
    let db = OptimisticTransactionDB::<MultiThreaded>::open_cf_descriptors(
        &opts,
        path,
        Column::ALL.iter().map(|col| ColumnFamilyDescriptor::new(col.rocksdb_name(), col.rocksdb_options())),
    )?;

    Ok((Arc::new(db), backup_hande))
}

/// This runs in anothr thread as the backup engine is not thread safe
fn spawn_backup_db_task(
    backup_dir: &Path,
    restore_from_latest_backup: bool,
    db_path: &Path,
    db_restored_cb: oneshot::Sender<()>,
    mut recv: mpsc::Receiver<BackupRequest>,
) -> Result<()> {
    let mut backup_opts = BackupEngineOptions::new(backup_dir).context("creating backup options")?;
    let cores = std::thread::available_parallelism().map(|e| e.get() as i32).unwrap_or(1);
    backup_opts.set_max_background_operations(cores);

    let mut engine = BackupEngine::open(&backup_opts, &Env::new().context("creating rocksdb env")?)
        .context("opening backup engine")?;

    if restore_from_latest_backup {
        log::info!("⏳ Restoring latest backup...");
        log::debug!("restore path is {db_path:?}");
        fs::create_dir_all(db_path).with_context(|| format!("creating directories {:?}", db_path))?;

        let opts = rocksdb::backup::RestoreOptions::default();
        engine.restore_from_latest_backup(db_path, db_path, &opts).context("restoring database")?;
        log::debug!("restoring latest backup done");
    }

    db_restored_cb.send(()).ok().context("receiver dropped")?;

    while let Some(BackupRequest { callback, db }) = recv.blocking_recv() {
        engine.create_new_backup_flush(&db, true).context("creating rocksdb backup")?;
        let _ = callback.send(());
    }

    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Meta,

    // Blocks storage
    // block_n => Block info
    BlockNToBlockInfo,
    // block_n => Block inner
    BlockNToBlockInner,
    /// Many To One
    TxHashToBlockN,
    /// One To One
    BlockHashToBlockN,
    /// Meta column for block storage (sync tip, pending block)
    BlockStorageMeta,

    /// Contract class hash to class data
    ContractClassData,

    CompiledContractClass,

    // History of contract class hashes
    // contract_address history block_number => class_hash
    ContractToClassHashes,

    // History of contract nonces
    // contract_address history block_number => nonce
    ContractToNonces,

    // Class hash => compiled class hash
    ContractClassHashes,

    // History of contract key => values
    // (contract_address, storage_key) history block_number => felt
    ContractStorage,
    /// Block number to state diff
    BlockStateDiff,

    // Each bonsai storage has 3 columns
    BonsaiContractsTrie,
    BonsaiContractsFlat,
    BonsaiContractsLog,

    BonsaiContractsStorageTrie,
    BonsaiContractsStorageFlat,
    BonsaiContractsStorageLog,

    BonsaiClassesTrie,
    BonsaiClassesFlat,
    BonsaiClassesLog,
}

impl Column {
    fn iter() -> impl Iterator<Item = Self> {
        Self::ALL.iter().copied()
    }
}

impl fmt::Debug for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.rocksdb_name())
    }
}

impl fmt::Display for Column {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.rocksdb_name())
    }
}

impl Column {
    pub const ALL: &'static [Self] = {
        use Column::*;
        &[
            Meta,
            BlockNToBlockInfo,
            BlockNToBlockInner,
            TxHashToBlockN,
            BlockHashToBlockN,
            BlockStorageMeta,
            ContractClassData,
            CompiledContractClass,
            ContractToClassHashes,
            ContractToNonces,
            ContractClassHashes,
            ContractStorage,
            BlockStateDiff,
            BonsaiContractsTrie,
            BonsaiContractsFlat,
            BonsaiContractsLog,
            BonsaiContractsStorageTrie,
            BonsaiContractsStorageFlat,
            BonsaiContractsStorageLog,
            BonsaiClassesTrie,
            BonsaiClassesFlat,
            BonsaiClassesLog,
        ]
    };
    pub const NUM_COLUMNS: usize = Self::ALL.len();

    pub(crate) fn rocksdb_name(&self) -> &'static str {
        use Column::*;
        match self {
            Meta => "meta",
            BlockNToBlockInfo => "block_n_to_block_info",
            BlockNToBlockInner => "block_n_to_block_inner",
            TxHashToBlockN => "tx_hash_to_block_n",
            BlockHashToBlockN => "block_hash_to_block_n",
            BlockStorageMeta => "block_storage_meta",
            BonsaiContractsTrie => "bonsai_contracts_trie",
            BonsaiContractsFlat => "bonsai_contracts_flat",
            BonsaiContractsLog => "bonsai_contracts_log",
            BonsaiContractsStorageTrie => "bonsai_contracts_storage_trie",
            BonsaiContractsStorageFlat => "bonsai_contracts_storage_flat",
            BonsaiContractsStorageLog => "bonsai_contracts_storage_log",
            BonsaiClassesTrie => "bonsai_classes_trie",
            BonsaiClassesFlat => "bonsai_classes_flat",
            BonsaiClassesLog => "bonsai_classes_log",
            BlockStateDiff => "block_state_diff",
            ContractClassData => "contract_class_data",
            CompiledContractClass => "compiled_contract_class",
            ContractToClassHashes => "contract_to_class_hashes",
            ContractToNonces => "contract_to_nonces",
            ContractClassHashes => "contract_class_hashes",
            ContractStorage => "contrac_storage",
        }
    }

    /// Per column rocksdb options, like memory budget, compaction profiles, block sizes for hdd/sdd
    /// etc. TODO: add basic sensible defaults
    pub(crate) fn rocksdb_options(&self) -> Options {
        let mut opts = Options::default();
        match self {
            Column::ContractStorage => {
                opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(
                    storage_handler::contract_storage::CONTRACT_STORAGE_PREFIX_EXTRACTOR,
                ));
            }
            Column::ContractToClassHashes => {
                opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(
                    storage_handler::contract_data::CONTRACT_CLASS_HASH_PREFIX_EXTRACTOR,
                ));
            }
            Column::ContractToNonces => {
                opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(
                    storage_handler::contract_data::CONTRACT_NONCES_PREFIX_EXTRACTOR,
                ));
            }
            _ => {}
        }
        opts
    }
}

pub trait DatabaseExt {
    fn get_column(&self, col: Column) -> Arc<BoundColumnFamily<'_>>;
}

impl DatabaseExt for DB {
    fn get_column(&self, col: Column) -> Arc<BoundColumnFamily<'_>> {
        let name = col.rocksdb_name();
        match self.cf_handle(name) {
            Some(column) => column,
            None => panic!("column {name} not initialized"),
        }
    }
}

/// Deoxys client database backend singleton.
#[derive(Debug)]
pub struct DeoxysBackend {
    mapping: Arc<MappingDb>,
    backup_handle: Option<mpsc::Sender<BackupRequest>>,
    db: Arc<DB>,
    last_flush_time: Mutex<Option<Instant>>,
}

pub struct DatabaseService {
    handle: Arc<DeoxysBackend>,
}

impl DatabaseService {
    pub async fn new(
        base_path: &Path,
        backup_dir: Option<PathBuf>,
        restore_from_latest_backup: bool,
    ) -> anyhow::Result<Self> {
        log::info!("💾 Opening database at: {}", base_path.display());

        let handle = DeoxysBackend::open(base_path.to_owned(), backup_dir.clone(), restore_from_latest_backup)
            .await
            .context("opening database")?;

        Ok(Self { handle })
    }

    pub fn backend(&self) -> &Arc<DeoxysBackend> {
        &self.handle
    }
}

struct BackupRequest {
    callback: oneshot::Sender<()>,
    db: Arc<DB>,
}

impl Drop for DeoxysBackend {
    fn drop(&mut self) {
        log::info!("⏳ Gracefully closing the database...");
    }
}

impl DeoxysBackend {
    /// Open the db.
    async fn open(
        db_config_dir: PathBuf,
        backup_dir: Option<PathBuf>,
        restore_from_latest_backup: bool,
    ) -> Result<Arc<DeoxysBackend>> {
        let db_path = db_config_dir.join("db");

        let (db, backup_handle) =
            open_rocksdb(&db_path, true, backup_dir, restore_from_latest_backup).await.context("opening database")?;

        let backend = Arc::new(Self {
            mapping: Arc::new(MappingDb::new(Arc::clone(&db))),
            backup_handle,
            db,
            last_flush_time: Default::default(),
        });

        Ok(backend)
    }

    pub fn maybe_flush(&self) -> Result<bool> {
        let mut inst = self.last_flush_time.lock().expect("poisoned mutex");
        let should_flush = match *inst {
            Some(inst) => inst.elapsed() >= Duration::from_secs(5),
            None => true,
        };
        if should_flush {
            log::debug!("doing a db flush");
            let mut opts = FlushOptions::default();
            opts.set_wait(true);
            // we have to collect twice here :/
            let columns = Column::ALL.iter().map(|e| self.db.get_column(*e)).collect::<Vec<_>>();
            let columns = columns.iter().collect::<Vec<_>>();
            self.db.flush_cfs_opt(&columns, &opts).context("flushing database")?;

            *inst = Some(Instant::now());
        }

        Ok(should_flush)
    }

    pub async fn backup(&self) -> Result<()> {
        let (callback_sender, callback_recv) = oneshot::channel();
        let _res = self
            .backup_handle
            .as_ref()
            .context("backups are not enabled")?
            .try_send(BackupRequest { callback: callback_sender, db: Arc::clone(&self.db) });
        callback_recv.await.context("backups task died :(")?;
        Ok(())
    }

    /// Return the mapping database manager
    pub fn mapping(&self) -> &Arc<MappingDb> {
        &self.mapping
    }

    pub fn expose_db(&self) -> &Arc<DB> {
        &self.db
    }

    pub fn contract_storage_mut(&self) -> ContractStorageViewMut {
        ContractStorageViewMut::new(Arc::clone(&self.db))
    }

    pub fn contract_storage(&self) -> ContractStorageView {
        ContractStorageView::new(Arc::clone(&self.db))
    }

    pub fn contract_class_data_mut(&self) -> ContractClassDataViewMut {
        ContractClassDataViewMut::new(Arc::clone(&self.db))
    }

    pub fn contract_class_data(&self) -> ContractClassDataView {
        ContractClassDataView::new(Arc::clone(&self.db))
    }

    pub fn compiled_contract_class_mut(&self) -> CompiledContractClassViewMut {
        CompiledContractClassViewMut::new(Arc::clone(&self.db))
    }

    pub fn compiled_contract_class(&self) -> CompiledContractClassView {
        CompiledContractClassView::new(Arc::clone(&self.db))
    }

    pub fn contract_class_hashes_mut(&self) -> ContractClassHashesViewMut {
        ContractClassHashesViewMut::new(Arc::clone(&self.db))
    }

    pub fn contract_class_hashes(&self) -> ContractClassHashesView {
        ContractClassHashesView::new(Arc::clone(&self.db))
    }

    pub fn contract_class_hash(&self) -> ContractClassView {
        ContractClassView::new(Arc::clone(&self.db))
    }

    pub fn contract_class_hash_mut(&self) -> ContractClassViewMut {
        ContractClassViewMut::new(Arc::clone(&self.db))
    }

    pub fn contract_nonces(&self) -> ContractNoncesView {
        ContractNoncesView::new(Arc::clone(&self.db))
    }

    pub fn contract_nonces_mut(&self) -> ContractNoncesViewMut {
        ContractNoncesViewMut::new(Arc::clone(&self.db))
    }

    pub fn block_state_diff(&self) -> BlockStateDiffView {
        BlockStateDiffView::new(Arc::clone(&self.db))
    }

    // tries

    pub(crate) fn get_bonsai<H: StarkHash + Send + Sync>(
        &self,
        map: DatabaseKeyMapping,
    ) -> BonsaiStorage<BasicId, BonsaiDb<'_>, H> {
        // UNWRAP: function actually cannot panic
        let bonsai = BonsaiStorage::new(
            BonsaiDb::new(&self.db, map),
            BonsaiStorageConfig {
                max_saved_trie_logs: Some(0),
                max_saved_snapshots: Some(0),
                snapshot_interval: u64::MAX,
            },
        )
        .unwrap();

        bonsai
    }

    pub(crate) fn bonsai_contract(&self) -> BonsaiStorage<BasicId, BonsaiDb<'_>, Pedersen> {
        self.get_bonsai(DatabaseKeyMapping {
            flat: Column::BonsaiContractsFlat,
            trie: Column::BonsaiContractsTrie,
            log: Column::BonsaiContractsLog,
        })
    }

    pub(crate) fn bonsai_storage(&self) -> BonsaiStorage<BasicId, BonsaiDb<'_>, Pedersen> {
        self.get_bonsai(DatabaseKeyMapping {
            flat: Column::BonsaiContractsStorageFlat,
            trie: Column::BonsaiContractsStorageTrie,
            log: Column::BonsaiContractsStorageLog,
        })
    }

    pub(crate) fn bonsai_class(&self) -> BonsaiStorage<BasicId, BonsaiDb<'_>, Poseidon> {
        self.get_bonsai(DatabaseKeyMapping {
            flat: Column::BonsaiClassesFlat,
            trie: Column::BonsaiClassesTrie,
            log: Column::BonsaiClassesLog,
        })
    }

    pub fn contract_trie_mut(&self) -> ContractTrieViewMut<'_> {
        ContractTrieViewMut(self.bonsai_contract())
    }

    pub fn contract_trie(&self) -> ContractTrieView<'_> {
        ContractTrieView(self.bonsai_contract())
    }

    pub fn contract_storage_trie_mut(&self) -> ContractStorageTrieViewMut<'_> {
        ContractStorageTrieViewMut(self.bonsai_storage())
    }

    pub fn contract_storage_trie(&self) -> ContractStorageTrieView<'_> {
        ContractStorageTrieView(self.bonsai_storage())
    }

    pub fn class_trie_mut(&self) -> ClassTrieViewMut<'_> {
        ClassTrieViewMut(self.bonsai_class())
    }

    pub fn class_trie(&self) -> ClassTrieView<'_> {
        ClassTrieView(self.bonsai_class())
    }
}
