pub mod gmail;
pub mod health;
pub mod notify;

pub use gmail::GmailConnector;
pub use health::{CheckStatus, HealthReport};
pub use notify::{LogNotifier, Notifier, NotifyLevel};

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("connector request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("connector returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("no send capacity available across {senders} senders")]
    NoCapacity { senders: usize },
    #[error("invalid recipient address: {0}")]
    InvalidRecipient(String),
    #[error("header {header} carried a line break")]
    HeaderInjection { header: &'static str },
    #[error("credential file problem at {path}: {reason}")]
    Credentials { path: String, reason: String },
    #[error("connector response shape unexpected: {0}")]
    Decode(String),
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadContext {
    pub thread_id: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub original_subject: Option<String>,
    pub prefer_sender: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundEmail {
    pub to: String,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
    pub thread: Option<ThreadContext>,
}

impl OutboundEmail {
    pub fn new(to: impl Into<String>, subject: impl Into<String>, body_text: impl Into<String>) -> Self {
        Self {
            to: to.into(),
            subject: subject.into(),
            body_text: body_text.into(),
            body_html: None,
            thread: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReceipt {
    pub message_id: String,
    pub thread_id: String,
    pub sender_email: String,
}

pub trait EmailTransport {
    fn send(&self, email: &OutboundEmail)
    -> impl Future<Output = Result<SendReceipt, ConnectorError>> + Send;
}

pub fn is_plausible_email(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !address.chars().any(char::is_whitespace)
        && address.matches('@').count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausible_emails_pass_and_junk_fails() {
        assert!(is_plausible_email("jane@acme.io"));
        assert!(is_plausible_email("j.doe+tag@sub.acme.co.uk"));
        assert!(!is_plausible_email("no-at-sign"));
        assert!(!is_plausible_email("two@@acme.io"));
        assert!(!is_plausible_email("@acme.io"));
        assert!(!is_plausible_email("jane@"));
        assert!(!is_plausible_email("jane@acme"));
        assert!(!is_plausible_email("jane doe@acme.io"));
        assert!(!is_plausible_email("jane@.acme.io"));
    }
}
