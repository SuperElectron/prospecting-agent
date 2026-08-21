use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTask {
    pub id: Uuid,
    pub kind: TaskKind,
    pub contact_id: Option<Uuid>,
    pub company_domain: Option<String>,
    pub payload: serde_json::Value,
    pub status: TaskStatus,
    pub attempts: u8,
    pub due_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl AgentTask {
    pub fn new(kind: TaskKind, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            contact_id: None,
            company_domain: None,
            payload,
            status: TaskStatus::Pending,
            attempts: 0,
            due_at: None,
            created_at: Utc::now(),
        }
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.status == TaskStatus::Pending && self.due_at.is_none_or(|due| due <= now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    EnrichContact,
    ResearchCompany,
    GenerateOutreach,
    SendOutreach,
    AnalyzeReply,
    NotifyRep,
    SyncCrm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn task_with_no_due_date_is_immediately_due() {
        let t = AgentTask::new(TaskKind::EnrichContact, serde_json::json!({}));
        assert!(t.is_due(Utc::now()));
    }

    #[test]
    fn future_due_date_defers_the_task() {
        let mut t = AgentTask::new(TaskKind::SendOutreach, serde_json::json!({}));
        t.due_at = Some(Utc::now() + Duration::hours(2));
        assert!(!t.is_due(Utc::now()));
        assert!(t.is_due(Utc::now() + Duration::hours(3)));
    }

    #[test]
    fn non_pending_tasks_are_never_due() {
        let mut t = AgentTask::new(TaskKind::AnalyzeReply, serde_json::json!({}));
        t.status = TaskStatus::Done;
        assert!(!t.is_due(Utc::now()));
    }
}
