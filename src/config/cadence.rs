use chrono::{DateTime, Datelike, Duration, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cadence {
    pub name: String,
    pub max_steps: u8,
    pub min_days_between: u8,
    pub send_days: Vec<Weekday>,
    pub send_hours: (u8, u8),
}

impl Cadence {
    pub fn standard() -> Self {
        Self {
            name: "standard".into(),
            max_steps: 3,
            min_days_between: 3,
            send_days: vec![Weekday::Tue, Weekday::Wed, Weekday::Thu],
            send_hours: (8, 16),
        }
    }

    pub fn gentle() -> Self {
        Self {
            name: "gentle".into(),
            max_steps: 2,
            min_days_between: 5,
            ..Self::standard()
        }
    }

    pub fn is_send_window(&self, at: DateTime<Utc>) -> bool {
        let hour = u8::try_from(at.hour()).unwrap_or(u8::MAX);
        self.send_days.contains(&at.weekday()) && hour >= self.send_hours.0 && hour < self.send_hours.1
    }

    pub fn earliest_next_send(&self, last_sent: DateTime<Utc>) -> DateTime<Utc> {
        let mut candidate = last_sent + Duration::days(i64::from(self.min_days_between));
        for _ in 0..14 {
            if self.is_send_window(candidate) {
                return candidate;
            }
            candidate += Duration::hours(1);
        }
        for _ in 0..14 {
            candidate += Duration::days(1);
            let aligned = candidate
                .with_hour(u32::from(self.send_hours.0))
                .unwrap_or(candidate);
            if self.is_send_window(aligned) {
                return aligned;
            }
        }
        candidate
    }
}

pub fn registry() -> Vec<Cadence> {
    vec![Cadence::standard(), Cadence::gentle()]
}

pub fn by_name(name: &str) -> Option<Cadence> {
    registry().into_iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn tuesday_morning_is_in_window() {
        let c = Cadence::standard();
        let tue_9am = Utc.with_ymd_and_hms(2026, 8, 18, 9, 0, 0).unwrap();
        assert_eq!(tue_9am.weekday(), Weekday::Tue);
        assert!(c.is_send_window(tue_9am));
    }

    #[test]
    fn weekend_and_evening_are_outside_window() {
        let c = Cadence::standard();
        let sat = Utc.with_ymd_and_hms(2026, 8, 22, 10, 0, 0).unwrap();
        assert_eq!(sat.weekday(), Weekday::Sat);
        assert!(!c.is_send_window(sat));
        let tue_8pm = Utc.with_ymd_and_hms(2026, 8, 18, 20, 0, 0).unwrap();
        assert!(!c.is_send_window(tue_8pm));
    }

    #[test]
    fn next_send_respects_min_gap_and_window() {
        let c = Cadence::standard();
        let sent_tue_9am = Utc.with_ymd_and_hms(2026, 8, 18, 9, 0, 0).unwrap();
        let next = c.earliest_next_send(sent_tue_9am);
        assert!(next >= sent_tue_9am + Duration::days(3));
        assert!(c.is_send_window(next));
    }

    #[test]
    fn registry_lookup_by_name() {
        assert_eq!(by_name("gentle").unwrap().max_steps, 2);
        assert!(by_name("nope").is_none());
    }
}
