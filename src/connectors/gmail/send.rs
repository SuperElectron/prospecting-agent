use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;

use crate::connectors::gmail::auth::{OauthClient, SenderAccount};
use crate::connectors::gmail::mime::{MimeParams, build_mime, encode_message, reply_subject};
use crate::connectors::{ConnectorError, EmailTransport, OutboundEmail, SendReceipt, is_plausible_email};
use crate::db;

const DEFAULT_API_BASE: &str = "https://gmail.googleapis.com";
const REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct SendResponse {
    id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
}

struct CachedToken {
    token: String,
    valid_until: Instant,
}

#[derive(Clone)]
pub struct GmailConnector {
    http: reqwest::Client,
    oauth: OauthClient,
    senders: Vec<SenderAccount>,
    pool: PgPool,
    api_base: String,
    cursor: Arc<AtomicUsize>,
    tokens: Arc<Mutex<HashMap<String, CachedToken>>>,
}

impl GmailConnector {
    pub fn new(oauth: OauthClient, senders: Vec<SenderAccount>, pool: PgPool) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("static gmail client configuration is valid");
        Self {
            http,
            oauth,
            senders,
            pool,
            api_base: DEFAULT_API_BASE.into(),
            cursor: Arc::new(AtomicUsize::new(0)),
            tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn bearer_for(&self, sender: &SenderAccount) -> Result<String, ConnectorError> {
        let mut cache = self.tokens.lock().await;
        if let Some(cached) = cache.get(&sender.email)
            && cached.valid_until > Instant::now()
        {
            return Ok(cached.token.clone());
        }
        let fresh = self
            .oauth
            .access_token_with_expiry(&self.http, &sender.refresh_token)
            .await?;
        let valid_until = Instant::now() + Duration::from_secs(fresh.expires_in_secs.saturating_sub(60));
        cache.insert(
            sender.email.clone(),
            CachedToken {
                token: fresh.token.clone(),
                valid_until,
            },
        );
        Ok(fresh.token)
    }

    #[must_use]
    pub fn with_api_base(mut self, api_base: &str) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_string();
        self
    }

    pub fn senders(&self) -> &[SenderAccount] {
        &self.senders
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn api_base(&self) -> &str {
        &self.api_base
    }

    async fn reserve_sender(&self, prefer: Option<&str>) -> Result<&SenderAccount, ConnectorError> {
        if self.senders.is_empty() {
            return Err(ConnectorError::NoSenders);
        }
        let day = Utc::now().date_naive();
        if let Some(email) = prefer
            && let Some(sender) = self.senders.iter().find(|s| s.email == email)
            && db::capacity::try_reserve(&self.pool, &sender.email, day, sender.daily_limit).await?
        {
            return Ok(sender);
        }
        let start = self.cursor.fetch_add(1, Ordering::Relaxed);
        for offset in 0..self.senders.len() {
            let sender = &self.senders[(start + offset) % self.senders.len()];
            if db::capacity::try_reserve(&self.pool, &sender.email, day, sender.daily_limit).await? {
                return Ok(sender);
            }
        }
        Err(ConnectorError::NoCapacity {
            senders: self.senders.len(),
        })
    }

    async fn post_send(
        &self,
        sender: &SenderAccount,
        raw: String,
        thread_id: Option<&str>,
    ) -> Result<SendReceipt, ConnectorError> {
        let token = self.bearer_for(sender).await?;
        let url = format!("{}/gmail/v1/users/me/messages/send", self.api_base);
        let mut body = serde_json::json!({ "raw": raw });
        if let Some(thread) = thread_id {
            body["threadId"] = serde_json::json!(thread);
        }
        let resp = self.http.post(&url).bearer_auth(token).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ConnectorError::Status { status, body });
        }
        let raw_body = resp.text().await?;
        let parsed: SendResponse =
            serde_json::from_str(&raw_body).map_err(|e| ConnectorError::Decode(e.to_string()))?;
        Ok(SendReceipt {
            message_id: parsed.id,
            thread_id: parsed.thread_id,
            sender_email: sender.email.clone(),
        })
    }
}

impl EmailTransport for GmailConnector {
    async fn send(&self, email: &OutboundEmail) -> Result<SendReceipt, ConnectorError> {
        if !is_plausible_email(&email.to) {
            return Err(ConnectorError::InvalidRecipient(email.to.clone()));
        }
        let prefer = email.thread.as_ref().and_then(|t| t.prefer_sender.as_deref());
        let sender = self.reserve_sender(prefer).await?;
        let boundary = format!("boundary_{}", uuid::Uuid::new_v4().simple());
        let subject = match &email.thread {
            Some(thread) if thread.in_reply_to.is_some() => {
                reply_subject(thread.original_subject.as_deref(), &email.subject)
            }
            _ => email.subject.clone(),
        };
        let empty: [String; 0] = [];
        let mime = build_mime(&MimeParams {
            to: &email.to,
            from: &sender.email,
            from_name: &sender.name,
            subject: &subject,
            body_text: &email.body_text,
            body_html: email.body_html.as_deref(),
            in_reply_to: email.thread.as_ref().and_then(|t| t.in_reply_to.as_deref()),
            references: email.thread.as_ref().map_or(&empty[..], |t| &t.references),
            boundary: &boundary,
        })?;
        let thread_id = email
            .thread
            .as_ref()
            .map(|t| t.thread_id.as_str())
            .filter(|t| !t.is_empty());
        self.post_send(sender, encode_message(&mime), thread_id).await
    }
}
