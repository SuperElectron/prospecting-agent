pub mod csv;
pub mod ingest;

pub use csv::{CsvSyncReport, RowSkip, sync_dir};
pub use ingest::{ingest_person, ingest_signal};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("memory error: {0}")]
    Memory(#[from] crate::memory::MemoryError),
    #[error("cannot read {path}: {reason}")]
    Io { path: String, reason: String },
    #[error("csv shape problem in {file}: {reason}")]
    Csv { file: String, reason: String },
}
