pub mod weekly;

pub use weekly::{ActivityReport, activity_report};

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
}
