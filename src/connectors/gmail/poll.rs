use serde::Deserialize;

use crate::connectors::ConnectorError;
use crate::connectors::gmail::send::GmailConnector;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MessageRef {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMessage {
    pub id: String,
    pub thread_id: String,
    pub from: Option<String>,
    pub subject: Option<String>,
    pub message_id_header: Option<String>,
    pub references: Vec<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    pub messages: Vec<MessageRef>,
    pub next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    messages: Vec<MessageRef>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct MessageResponse {
    id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
    #[serde(default)]
    snippet: String,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

impl GmailConnector {
    pub async fn list_messages(
        &self,
        sender_email: &str,
        query: &str,
        max_results: u16,
        page_token: Option<&str>,
    ) -> Result<MessagePage, ConnectorError> {
        let sender = self.sender_account(sender_email)?;
        let token = self.bearer_for(sender).await?;
        let url = format!("{}/gmail/v1/users/me/messages", self.api_base());
        let max = max_results.to_string();
        let mut params = vec![("q", query), ("maxResults", max.as_str())];
        if let Some(token_value) = page_token {
            params.push(("pageToken", token_value));
        }
        let resp = self
            .http()
            .get(&url)
            .bearer_auth(token)
            .query(&params)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ConnectorError::Status { status, body });
        }
        let raw = resp.text().await?;
        let parsed: ListResponse =
            serde_json::from_str(&raw).map_err(|e| ConnectorError::Decode(e.to_string()))?;
        Ok(MessagePage {
            messages: parsed.messages,
            next_page_token: parsed.next_page_token,
        })
    }

    pub async fn fetch_message(
        &self,
        sender_email: &str,
        message_id: &str,
    ) -> Result<InboundMessage, ConnectorError> {
        let sender = self.sender_account(sender_email)?;
        let token = self.bearer_for(sender).await?;
        let url = format!("{}/gmail/v1/users/me/messages/{message_id}", self.api_base());
        let resp = self
            .http()
            .get(&url)
            .bearer_auth(token)
            .query(&[
                ("format", "metadata"),
                ("metadataHeaders", "From"),
                ("metadataHeaders", "Subject"),
                ("metadataHeaders", "Message-ID"),
                ("metadataHeaders", "References"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ConnectorError::Status { status, body });
        }
        let raw = resp.text().await?;
        let parsed: MessageResponse =
            serde_json::from_str(&raw).map_err(|e| ConnectorError::Decode(e.to_string()))?;
        let headers = parsed.payload.map(|p| p.headers).unwrap_or_default();
        let header = |name: &str| {
            headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| h.value.clone())
        };
        Ok(InboundMessage {
            id: parsed.id,
            thread_id: parsed.thread_id,
            from: header("From"),
            subject: header("Subject"),
            message_id_header: header("Message-ID"),
            references: header("References")
                .map(|r| r.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            snippet: parsed.snippet,
        })
    }

    fn sender_account(&self, email: &str) -> Result<&super::auth::SenderAccount, ConnectorError> {
        self.senders()
            .iter()
            .find(|s| s.email == email)
            .ok_or_else(|| ConnectorError::Auth(format!("no configured sender named {email}")))
    }
}
