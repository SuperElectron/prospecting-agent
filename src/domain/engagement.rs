use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Engagement {
    pub id: Uuid,
    pub contact_id: Uuid,
    pub channel: Channel,
    pub direction: Direction,
    pub kind: EngagementKind,
    pub subject: Option<String>,
    pub body: Option<String>,
    pub sequence_step: Option<u8>,
    pub occurred_at: DateTime<Utc>,
}

impl Engagement {
    pub fn outbound(contact_id: Uuid, channel: Channel, kind: EngagementKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            contact_id,
            channel,
            direction: Direction::Outbound,
            kind,
            subject: None,
            body: None,
            sequence_step: None,
            occurred_at: Utc::now(),
        }
    }

    pub fn inbound(contact_id: Uuid, channel: Channel, kind: EngagementKind) -> Self {
        Self {
            direction: Direction::Inbound,
            ..Self::outbound(contact_id, channel, kind)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Email,
    Linkedin,
    Slack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outbound,
    Inbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngagementKind {
    Sent,
    Delivered,
    Opened,
    Clicked,
    Replied,
    Bounced,
    ConnectionRequest,
    ConnectionAccepted,
    Message,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SequenceState {
    pub contact_id: Uuid,
    pub cadence: String,
    pub current_step: u8,
    pub max_steps: u8,
    pub last_sent_at: Option<DateTime<Utc>>,
    pub stopped: bool,
    pub stop_reason: Option<StopReason>,
}

impl SequenceState {
    pub fn start(contact_id: Uuid, cadence: impl Into<String>, max_steps: u8) -> Self {
        Self {
            contact_id,
            cadence: cadence.into(),
            current_step: 0,
            max_steps,
            last_sent_at: None,
            stopped: false,
            stop_reason: None,
        }
    }

    pub fn advance(&mut self) {
        if self.stopped {
            return;
        }
        if self.current_step < self.max_steps {
            self.current_step += 1;
            self.last_sent_at = Some(Utc::now());
        }
        if self.current_step >= self.max_steps {
            self.stop(StopReason::Completed);
        }
    }

    pub fn stop(&mut self, reason: StopReason) {
        self.stopped = true;
        self.stop_reason = Some(reason);
    }

    pub fn is_active(&self) -> bool {
        !self.stopped
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    Replied,
    OptedOut,
    Bounced,
    Manual,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_advances_then_completes_at_max() {
        let mut s = SequenceState::start(Uuid::new_v4(), "default", 3);
        assert!(s.is_active());
        s.advance();
        s.advance();
        assert!(s.is_active());
        assert_eq!(s.current_step, 2);
        s.advance();
        assert!(!s.is_active());
        assert_eq!(s.stop_reason, Some(StopReason::Completed));
    }

    #[test]
    fn stopped_sequence_does_not_advance() {
        let mut s = SequenceState::start(Uuid::new_v4(), "default", 3);
        s.advance();
        s.stop(StopReason::Replied);
        let step = s.current_step;
        s.advance();
        assert_eq!(s.current_step, step);
        assert_eq!(s.stop_reason, Some(StopReason::Replied));
    }

    #[test]
    fn terminal_stop_reason_survives_further_advances() {
        let mut s = SequenceState::start(Uuid::new_v4(), "default", 2);
        s.advance();
        s.advance();
        assert_eq!(s.stop_reason, Some(StopReason::Completed));
        s.stop(StopReason::OptedOut);
        s.advance();
        assert_eq!(s.stop_reason, Some(StopReason::OptedOut));
    }

    #[test]
    fn inbound_flips_direction_only() {
        let id = Uuid::new_v4();
        let e = Engagement::inbound(id, Channel::Email, EngagementKind::Replied);
        assert_eq!(e.direction, Direction::Inbound);
        assert_eq!(e.contact_id, id);
    }

    #[test]
    fn serde_roundtrip_preserves_engagement() {
        let e = Engagement::outbound(Uuid::new_v4(), Channel::Linkedin, EngagementKind::Sent);
        let json = serde_json::to_string(&e).unwrap();
        let back: Engagement = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
