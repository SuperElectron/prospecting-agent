use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::config::{Cadence, MessagingRules, cadence};
use crate::connectors::{EmailTransport, OutboundEmail};
use crate::db;
use crate::domain::{Channel, Contact, ContactStatus, Engagement, EngagementKind, SequenceState, StopReason};
use crate::llm::{LlmClient, Policy};
use crate::memory::{EntityRef, MemoryClient};
use crate::workflows::accounts::{PreflightConfig, PreflightDecision, preflight};
use crate::workflows::outreach::OutreachError;
use crate::workflows::outreach::email::{EmailContext, generate_email};

#[derive(Debug, Default, Serialize)]
pub struct EnrollmentReport {
    pub enrolled: u64,
    pub skipped_uncontactable: u64,
    pub already_sequenced: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct SendPassReport {
    pub considered: u64,
    pub sent: u64,
    pub preflight_modified: u64,
    pub record_failed: u64,
    pub drafted_dry_run: u64,
    pub outside_window: u64,
    pub waiting_cadence: u64,
    pub completed: u64,
    pub stopped_uncontactable: u64,
    pub preflight_blocked: u64,
    pub preflight_delayed: u64,
    pub generation_failed: u64,
    pub send_failed: u64,
    pub missing_contact: u64,
    pub unknown_cadence: u64,
}

pub async fn enroll_contacts(
    pool: &PgPool,
    cadence: &Cadence,
    limit: i64,
) -> Result<EnrollmentReport, OutreachError> {
    let mut report = EnrollmentReport::default();
    let contacts = db::contacts::list_by_status(pool, ContactStatus::Enriched, limit).await?;
    for contact in contacts {
        if !contact.is_contactable() {
            report.skipped_uncontactable += 1;
            continue;
        }
        if db::sequences::for_contact(pool, contact.id).await?.is_some() {
            report.already_sequenced += 1;
            continue;
        }
        let state = SequenceState::start(contact.id, cadence.name.clone(), cadence.max_steps);
        db::sequences::upsert(pool, &state).await?;
        db::contacts::advance_status(
            pool,
            contact.id,
            ContactStatus::Enriched,
            ContactStatus::InSequence,
        )
        .await?;
        report.enrolled += 1;
    }
    Ok(report)
}

pub struct SendPassInputs<'a, T: EmailTransport> {
    pub memory: &'a MemoryClient,
    pub llm: &'a LlmClient,
    pub policies: &'a [Policy],
    pub rules: &'a MessagingRules,
    pub preflight_config: &'a PreflightConfig,
    pub transport: Option<&'a T>,
    pub dry_run: bool,
    pub limit: i64,
}

pub async fn run_send_pass<T: EmailTransport>(
    pool: &PgPool,
    inputs: &SendPassInputs<'_, T>,
    now: DateTime<Utc>,
) -> Result<SendPassReport, OutreachError> {
    let mut report = SendPassReport::default();
    if !inputs.dry_run && inputs.transport.is_none() {
        return Err(OutreachError::NoTransport);
    }
    let states = db::sequences::list_active(pool, inputs.limit).await?;
    for mut state in states {
        report.considered += 1;
        let Some(contact) = db::contacts::by_id(pool, state.contact_id).await? else {
            report.missing_contact += 1;
            continue;
        };
        if !contact.is_contactable() {
            let reason = if contact.status == ContactStatus::OptedOut {
                StopReason::OptedOut
            } else {
                StopReason::Manual
            };
            stop_sequence(pool, &mut state, reason).await?;
            report.stopped_uncontactable += 1;
            continue;
        }
        if state.current_step >= state.max_steps {
            stop_sequence(pool, &mut state, StopReason::Completed).await?;
            report.completed += 1;
            continue;
        }
        let Some(cadence) = cadence::by_name(&state.cadence) else {
            report.unknown_cadence += 1;
            continue;
        };
        if !due_now(&cadence, &state, now) {
            if cadence.is_send_window(now) {
                report.waiting_cadence += 1;
            } else {
                report.outside_window += 1;
            }
            continue;
        }
        if !clear_preflight(pool, inputs, &contact, &mut state, &mut report).await? {
            continue;
        }
        deliver(pool, inputs, &contact, &mut state, &mut report, now).await?;
    }
    Ok(report)
}

async fn clear_preflight<T: EmailTransport>(
    pool: &PgPool,
    inputs: &SendPassInputs<'_, T>,
    contact: &Contact,
    state: &mut SequenceState,
    report: &mut SendPassReport,
) -> Result<bool, OutreachError> {
    match preflight(pool, inputs.preflight_config, contact).await? {
        PreflightDecision::Proceed => Ok(true),
        PreflightDecision::Modify { cadence, max_emails } => {
            tracing::info!(
                contact = %contact.id,
                cadence,
                max_emails,
                "preflight modified the sequence; capping emails (cadence swap lands with the config lift)"
            );
            report.preflight_modified += 1;
            if max_emails < state.max_steps {
                state.max_steps = max_emails;
                db::sequences::upsert(pool, &*state).await?;
                if state.current_step >= state.max_steps {
                    stop_sequence(pool, state, StopReason::Completed).await?;
                    report.completed += 1;
                    return Ok(false);
                }
            }
            Ok(true)
        }
        PreflightDecision::Delay { until, reason } => {
            tracing::info!(contact = %contact.id, %until, reason, "outreach delayed by preflight");
            report.preflight_delayed += 1;
            Ok(false)
        }
        PreflightDecision::Block { reason } => {
            tracing::info!(contact = %contact.id, reason, "outreach blocked by preflight");
            stop_sequence(pool, state, StopReason::Manual).await?;
            report.preflight_blocked += 1;
            Ok(false)
        }
    }
}

async fn deliver<T: EmailTransport>(
    pool: &PgPool,
    inputs: &SendPassInputs<'_, T>,
    contact: &Contact,
    state: &mut SequenceState,
    report: &mut SendPassReport,
    now: DateTime<Utc>,
) -> Result<(), OutreachError> {
    let context = EmailContext {
        step: state.current_step + 1,
        max_steps: state.max_steps,
        is_first_touch: state.current_step == 0,
    };
    let email = match generate_email(
        pool,
        inputs.memory,
        inputs.llm,
        inputs.policies,
        inputs.rules,
        contact,
        context,
    )
    .await
    {
        Ok(email) => email,
        Err(e) => {
            tracing::warn!(contact = %contact.id, error = %e, "outreach generation failed");
            report.generation_failed += 1;
            return Ok(());
        }
    };
    if inputs.dry_run {
        tracing::info!(
            contact = %contact.id,
            subject = email.subject,
            "dry run: draft ready, not sending"
        );
        report.drafted_dry_run += 1;
        return Ok(());
    }
    let Some(transport) = inputs.transport else {
        return Err(OutreachError::NoTransport);
    };
    let address = contact.email.clone().ok_or(OutreachError::NoEmail(contact.id))?;
    let outbound = OutboundEmail {
        to: address,
        subject: email.subject.clone(),
        body_text: email.body_text.clone(),
        body_html: Some(email.body_html.clone()),
        thread: None,
    };
    let step = state.current_step + 1;
    state.advance(now);
    db::sequences::upsert(pool, &*state).await?;
    if let Err(e) = transport.send(&outbound).await {
        tracing::warn!(contact = %contact.id, error = %e, "outreach send failed; cadence slot burned");
        report.send_failed += 1;
        return Ok(());
    }
    report.sent += 1;
    if let Err(e) = record_sent(pool, inputs.memory, contact, &email, step).await {
        tracing::warn!(contact = %contact.id, error = %e, "sent email could not be recorded");
        report.record_failed += 1;
    }
    Ok(())
}

fn due_now(cadence: &Cadence, state: &SequenceState, now: DateTime<Utc>) -> bool {
    if !cadence.is_send_window(now) {
        return false;
    }
    match state.last_sent_at {
        Some(last) => cadence.earliest_next_send(last).is_some_and(|next| next <= now),
        None => true,
    }
}

async fn stop_sequence(
    pool: &PgPool,
    state: &mut SequenceState,
    reason: StopReason,
) -> Result<(), OutreachError> {
    state.stop(reason);
    db::sequences::upsert(pool, &*state).await?;
    Ok(())
}

async fn record_sent(
    pool: &PgPool,
    memory: &MemoryClient,
    contact: &Contact,
    email: &crate::workflows::outreach::GeneratedEmail,
    step: u8,
) -> Result<(), OutreachError> {
    let mut engagement = Engagement::outbound(contact.id, Channel::Email, EngagementKind::Sent);
    engagement.subject = Some(email.subject.clone());
    engagement.sequence_step = Some(step);
    db::engagements::insert(pool, &engagement).await?;
    crate::workflows::util::best_effort_memorize(
        memory,
        &EntityRef::Contact(contact.id),
        &format!("[SENT step {step}] angle used: {}", email.personalization_fact),
    )
    .await;
    Ok(())
}
