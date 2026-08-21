pub mod preflight;
pub mod strategy;

pub use preflight::{PreflightConfig, PreflightDecision, preflight};
pub use strategy::evaluate_account_strategy;

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("memory error: {0}")]
    Memory(#[from] crate::memory::MemoryError),
    #[error("llm error: {0}")]
    Llm(#[from] crate::llm::LlmError),
    #[error("company {0} is not in the database")]
    UnknownCompany(String),
}
