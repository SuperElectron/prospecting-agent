use serde::Serialize;
use sqlx::{PgPool, Row};

use crate::db::DbError;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct WeeklyReport {
    pub emails_sent: i64,
    pub replies: i64,
    pub opt_outs: i64,
    pub contacts_added: i64,
    pub companies_added: i64,
    pub signals_detected: i64,
    pub sequences_stopped: i64,
}

impl WeeklyReport {
    pub fn render(&self) -> String {
        format!(
            "weekly report: {sent} emails sent, {replies} replies, {optouts} opt-outs, \
             {contacts} contacts added, {companies} companies added, {signals} signals detected, \
             {stopped} sequences stopped",
            sent = self.emails_sent,
            replies = self.replies,
            optouts = self.opt_outs,
            contacts = self.contacts_added,
            companies = self.companies_added,
            signals = self.signals_detected,
            stopped = self.sequences_stopped,
        )
    }
}

pub async fn weekly_report(pool: &PgPool, window_days: i32) -> Result<WeeklyReport, DbError> {
    let days = window_days.max(1);
    let row = sqlx::query(
        "SELECT \
         (SELECT COUNT(*) FROM engagements WHERE direction = 'outbound' AND kind = 'sent' \
          AND occurred_at > now() - make_interval(days => $1)) AS emails_sent, \
         (SELECT COUNT(*) FROM engagements WHERE direction = 'inbound' AND kind = 'replied' \
          AND occurred_at > now() - make_interval(days => $1)) AS replies, \
         (SELECT COUNT(*) FROM contacts WHERE status = 'opted_out' \
          AND updated_at > now() - make_interval(days => $1)) AS opt_outs, \
         (SELECT COUNT(*) FROM contacts WHERE created_at > now() - make_interval(days => $1)) \
          AS contacts_added, \
         (SELECT COUNT(*) FROM companies WHERE created_at > now() - make_interval(days => $1)) \
          AS companies_added, \
         (SELECT COUNT(*) FROM signals WHERE detected_at > now() - make_interval(days => $1)) \
          AS signals_detected, \
         (SELECT COUNT(*) FROM sequence_states WHERE stopped = true \
          AND stopped_at > now() - make_interval(days => $1)) AS sequences_stopped",
    )
    .bind(days)
    .fetch_one(pool)
    .await?;
    Ok(WeeklyReport {
        emails_sent: row.get("emails_sent"),
        replies: row.get("replies"),
        opt_outs: row.get("opt_outs"),
        contacts_added: row.get("contacts_added"),
        companies_added: row.get("companies_added"),
        signals_detected: row.get("signals_detected"),
        sequences_stopped: row.get("sequences_stopped"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_reads_as_one_line() {
        let report = WeeklyReport {
            emails_sent: 12,
            replies: 3,
            ..WeeklyReport::default()
        };
        let line = report.render();
        assert!(line.contains("12 emails sent"));
        assert!(line.contains("3 replies"));
        assert!(!line.contains('\n'));
    }
}
