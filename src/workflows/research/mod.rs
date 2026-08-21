pub mod company;
pub mod signals;

pub use company::{ResearchOutcome, research_company};
pub use signals::{SignalDetectionReport, detect_signals};

#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("memory error: {0}")]
    Memory(#[from] crate::memory::MemoryError),
    #[error("search error: {0}")]
    Search(#[from] crate::clients::TavilyError),
    #[error("llm error: {0}")]
    Llm(#[from] crate::llm::LlmError),
    #[error("company {0} is not in the database")]
    UnknownCompany(String),
}
