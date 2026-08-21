use serde::Serialize;

use crate::db;
use crate::jobs::{JobContext, JobError};
use crate::workflows::analysis::{InboundReply, analyze_reply};

const POLL_QUERY: &str = "in:inbox -from:me newer_than:7d";
const POLL_PAGE_SIZE: u16 = 25;

#[derive(Debug, Default, Serialize)]
pub struct ReplyMonitorReport {
    pub senders_polled: u64,
    pub messages_listed: u64,
    pub replies_processed: u64,
    pub unmatched: u64,
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
        let page = match gmail
            .list_messages(&sender_email, POLL_QUERY, POLL_PAGE_SIZE, None)
            .await
        {
            Ok(page) => page,
            Err(e) => {
                tracing::warn!(sender = sender_email, error = %e, "reply listing failed");
                report.failures += 1;
                continue;
            }
        };
        for reference in page.messages {
            report.messages_listed += 1;
            if !db::inbound::claim_message(&ctx.pool, &reference.id, &sender_email).await? {
                report.already_processed += 1;
                continue;
            }
            match process_message(ctx, &sender_email, &reference.id).await {
                Ok(Processed::Reply) => report.replies_processed += 1,
                Ok(Processed::Unmatched) => report.unmatched += 1,
                Err(e) => {
                    tracing::warn!(message = reference.id, error = %e, "reply processing failed");
                    db::inbound::release_message(&ctx.pool, &reference.id).await?;
                    report.failures += 1;
                }
            }
        }
    }
    Ok(report)
}

enum Processed {
    Reply,
    Unmatched,
}

async fn process_message(
    ctx: &JobContext,
    sender_email: &str,
    message_id: &str,
) -> Result<Processed, String> {
    let gmail = ctx.gmail.as_ref().ok_or("gmail became unavailable")?;
    let message = gmail
        .fetch_message(sender_email, message_id)
        .await
        .map_err(|e| e.to_string())?;
    let Some(address) = message.from.as_deref().and_then(parse_address) else {
        return Ok(Processed::Unmatched);
    };
    let Some(contact) = db::contacts::by_email(&ctx.pool, address)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(Processed::Unmatched);
    };
    db::inbound::attach_contact(&ctx.pool, message_id, contact.id)
        .await
        .map_err(|e| e.to_string())?;
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
    .await
    .map_err(|e| e.to_string())?;
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
