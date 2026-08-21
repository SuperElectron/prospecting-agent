pub mod contacts;
pub mod enrich;

pub use crate::config::DiscoveryBudget;
pub use contacts::{DiscoveryReport, discover_contacts};
pub use enrich::{EnrichmentReport, enrich_companies, enrich_contacts};

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("ingest error: {0}")]
    Ingest(#[from] crate::workflows::sync::SyncError),
}
