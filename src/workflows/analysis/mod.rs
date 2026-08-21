pub mod reply;

pub use reply::{InboundReply, ReplyOutcome, analyze_reply};

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("memory error: {0}")]
    Memory(#[from] crate::memory::MemoryError),
    #[error("llm error: {0}")]
    Llm(#[from] crate::llm::LlmError),
    #[error("notify error: {0}")]
    Notify(#[from] crate::connectors::ConnectorError),
}
