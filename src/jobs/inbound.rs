use serde::Serialize;

use crate::db;
use crate::jobs::{JobContext, JobError};
use crate::workflows::analysis::{InboundReply, analyze_reply};

const POLL_QUERY: &str = "in:inbox -from:me newer_than:7d";
const POLL_PAGE_SIZE: u16 = 25;
const POLL_MAX_PAGES: u8 = 4;

#[derive(Debug, thiserror::Error)]
pub enum InboundError {
    #[error("gmail error: {0}")]
    Gmail(#[from] crate::connectors::ConnectorError),
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("analysis error: {0}")]
    Analysis(#[from] crate::workflows::analysis::AnalysisError),
    #[error("gmail became unavailable mid-poll")]
    GmailGone,
}

#[derive(Debug, Default, Serialize)]
pub struct ReplyMonitorReport {
    pub senders_polled: u64,
    pub pages_listed: u64,
    pub messages_listed: u64,
    pub replies_processed: u64,
    pub unparsed_from: u64,
    pub unknown_sender: u64,
    pub already_processed: u64,
    pub failures: u64,
}

pub fn parse_address(raw: &str) -> Option<&str> {
    let inner = match (raw.find('<'), raw.rfind('>')) {
        (Some(start), Some(end)) if end > start => &raw[start + 1..end],
        _ => raw,
    };
    let trimmed = inner.trim();
    if trimmed.contains('@') && !trimmed.contains(char::is_whitespace) {
        Some(trimmed)
    } else {
        None
    }
}

pub async fn poll_replies(ctx: &JobContext) -> Result<ReplyMonitorReport, JobError> {
    let mut report = ReplyMonitorReport::default();
    let Some(gmail) = ctx.gmail.as_ref() else {
        tracing::info!("reply monitor: gmail not configured, nothing to poll");
        return Ok(report);
    };
    let senders: Vec<String> = gmail
        .senders()
        .iter()
        .map(|sender| sender.email.clone())
        .collect();
    for sender_email in senders {
        report.senders_polled += 1;
        let mut page_token: Option<String> = None;
        for _ in 0..POLL_MAX_PAGES {
            let page = match gmail
                .list_messages(&sender_email, POLL_QUERY, POLL_PAGE_SIZE, page_token.as_deref())
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    tracing::warn!(sender = sender_email, error = %e, "reply listing failed");
                    report.failures += 1;
                    break;
                }
            };
            report.pages_listed += 1;
            for reference in page.messages {
                report.messages_listed += 1;
                handle_message(ctx, &sender_email, &reference.id, &mut report).await;
            }
            page_token = page.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
    }
    Ok(report)
}

async fn handle_message(
    ctx: &JobContext,
    sender_email: &str,
    message_id: &str,
    report: &mut ReplyMonitorReport,
) {
    match db::inbound::claim_message(&ctx.pool, message_id, sender_email).await {
        Ok(true) => {}
        Ok(false) => {
            report.already_processed += 1;
            return;
        }
        Err(e) => {
            tracing::warn!(message = message_id, error = %e, "message claim failed");
            report.failures += 1;
            return;
        }
    }
    match process_message(ctx, sender_email, message_id).await {
        Ok(outcome) => {
            match outcome {
                Processed::Reply => report.replies_processed += 1,
                Processed::UnparsedFrom => report.unparsed_from += 1,
                Processed::UnknownSender => report.unknown_sender += 1,
            }
            if let Err(e) = db::inbound::mark_completed(&ctx.pool, message_id).await {
                tracing::warn!(message = message_id, error = %e, "completion mark failed; lease will re-run it");
            }
        }
        Err(e) => {
            tracing::warn!(message = message_id, error = %e, "reply processing failed");
            report.failures += 1;
            if let Err(release_error) = db::inbound::release_message(&ctx.pool, message_id).await {
                tracing::warn!(
                    message = message_id,
                    error = %release_error,
                    "claim release failed; lease will re-run it"
                );
            }
        }
    }
}

enum Processed {
    Reply,
    UnparsedFrom,
    UnknownSender,
}

async fn process_message(
    ctx: &JobContext,
    sender_email: &str,
    message_id: &str,
) -> Result<Processed, InboundError> {
    let gmail = ctx.gmail.as_ref().ok_or(InboundError::GmailGone)?;
    let message = gmail.fetch_message(sender_email, message_id).await?;
    let Some(address) = message.from.as_deref().and_then(parse_address) else {
        return Ok(Processed::UnparsedFrom);
    };
    let Some(contact) = db::contacts::by_email(&ctx.pool, address).await? else {
        return Ok(Processed::UnknownSender);
    };
    db::inbound::attach_contact(&ctx.pool, message_id, contact.id).await?;
    analyze_reply(
        &ctx.pool,
        &ctx.memory,
        &ctx.llm,
        &ctx.policies,
        &ctx.notifier,
        &contact,
        InboundReply {
            subject: message.subject.as_deref(),
            body: &message.snippet,
        },
    )
    .await?;
    Ok(Processed::Reply)
}

#[cfg(test)]
mod tests {
    use super::parse_address;

    #[test]
    fn addresses_parse_from_display_forms() {
        assert_eq!(parse_address("Jane Doe <jane@acme.io>"), Some("jane@acme.io"));
        assert_eq!(parse_address("jane@acme.io"), Some("jane@acme.io"));
        assert_eq!(parse_address("  <j.d@x.co>  "), Some("j.d@x.co"));
        assert_eq!(parse_address("no address here"), None);
        assert_eq!(parse_address("broken <not an email>"), None);
    }
}
