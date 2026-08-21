use sqlx::PgPool;
use uuid::Uuid;

use crate::clients::apollo::ApolloPerson;
use crate::clients::{company_from_organization, contact_from_person};
use crate::db;
use crate::domain::Signal;
use crate::memory::{EntityRef, MemoryClient, MemoryError};
use crate::workflows::sync::SyncError;

pub async fn ingest_person(
    pool: &PgPool,
    memory: &MemoryClient,
    person: &ApolloPerson,
) -> Result<Uuid, SyncError> {
    let contact = contact_from_person(person);
    let contact_id = db::contacts::upsert(pool, &contact).await?;
    if let Some(company) = person.organization.as_ref().and_then(company_from_organization) {
        db::companies::upsert(pool, &company).await?;
    }
    let line = enrichment_line(contact.title.as_deref(), contact.company_domain.as_deref());
    best_effort_memorize(memory, &EntityRef::Contact(contact_id), &line).await;
    Ok(contact_id)
}

pub async fn ingest_signal(pool: &PgPool, memory: &MemoryClient, signal: &Signal) -> Result<(), SyncError> {
    db::signals::upsert(pool, signal).await?;
    let line = format!(
        "[SIGNAL {kind} / {strength}] {summary}",
        kind = signal_kind_label(signal),
        strength = signal_strength_label(signal),
        summary = signal.summary,
    );
    best_effort_memorize(memory, &EntityRef::company(&signal.company_domain), &line).await;
    Ok(())
}

fn enrichment_line(title: Option<&str>, company_domain: Option<&str>) -> String {
    let title = title.unwrap_or("unknown title");
    match company_domain {
        Some(domain) => format!("[ENRICHED apollo] {title} at {domain}"),
        None => format!("[ENRICHED apollo] {title}"),
    }
}

async fn best_effort_memorize(memory: &MemoryClient, entity: &EntityRef, line: &str) {
    match memory.memorize(entity, line, false).await {
        Ok(()) => {}
        Err(MemoryError::Backend(reason)) => {
            tracing::warn!(entity = %entity, reason, "memory backend rejected enrichment line");
        }
        Err(other) => {
            tracing::warn!(entity = %entity, error = %other, "memorize failed for enrichment line");
        }
    }
}

fn signal_kind_label(signal: &Signal) -> String {
    serde_json::to_value(signal.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "other".into())
}

fn signal_strength_label(signal: &Signal) -> String {
    serde_json::to_value(signal.strength)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "weak".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SignalKind, SignalStrength};

    #[test]
    fn enrichment_lines_read_well_with_and_without_a_domain() {
        assert_eq!(
            enrichment_line(Some("VP Sales"), Some("acme.io")),
            "[ENRICHED apollo] VP Sales at acme.io"
        );
        assert_eq!(enrichment_line(None, None), "[ENRICHED apollo] unknown title");
    }

    #[test]
    fn signal_labels_use_wire_names() {
        let signal = Signal::new(
            "acme.io",
            SignalKind::LeadershipChange,
            SignalStrength::Strong,
            "new CRO",
        );
        assert_eq!(signal_kind_label(&signal), "leadership_change");
        assert_eq!(signal_strength_label(&signal), "strong");
    }
}
