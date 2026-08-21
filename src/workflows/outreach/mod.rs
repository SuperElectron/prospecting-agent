pub mod email;
pub mod html;

pub use email::{EmailContext, GeneratedEmail, generate_email};
pub use html::render_html;

#[derive(Debug, thiserror::Error)]
pub enum OutreachError {
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("memory error: {0}")]
    Memory(#[from] crate::memory::MemoryError),
    #[error("llm error: {0}")]
    Llm(#[from] crate::llm::LlmError),
    #[error("contact {0} has no email address")]
    NoEmail(uuid::Uuid),
    #[error("generated email kept violating messaging rules: {0:?}")]
    RulesViolated(Vec<crate::config::MessagingViolation>),
}
