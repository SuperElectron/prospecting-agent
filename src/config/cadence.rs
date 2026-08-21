use chrono::{DateTime, Datelike, Duration, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};

const SEARCH_HORIZON_HOURS: i64 = 24 * 14;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CadenceError {
    #[error("cadence {0} has no send days")]
    NoSendDays(String),
    #[error("cadence {name} has invalid send hours {start}..{end}")]
    InvalidHours { name: String, start: u8, end: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendHours {
    pub start: u8,
    pub end: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cadence {
    pub name: String,
    pub max_steps: u8,
    pub min_days_between: u8,
    pub send_days: Vec<Weekday>,
    pub send_hours: SendHours,
}

impl Cadence {
    pub fn standard() -> Self {
        Self {
            name: "standard".into(),
            max_steps: 3,
            min_days_between: 3,
            send_days: vec![Weekday::Tue, Weekday::Wed, Weekday::Thu],
            send_hours: SendHours { start: 8, end: 16 },
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

    pub fn validate(&self) -> Result<(), CadenceError> {
        if self.send_days.is_empty() {
            return Err(CadenceError::NoSendDays(self.name.clone()));
        }
        if self.send_hours.start >= self.send_hours.end || self.send_hours.end > 24 {
            return Err(CadenceError::InvalidHours {
                name: self.name.clone(),
                start: self.send_hours.start,
                end: self.send_hours.end,
            });
        }
        Ok(())
    }

    pub fn is_send_window(&self, at: DateTime<Utc>) -> bool {
        self.send_days.contains(&at.weekday())
            && at.hour() >= u32::from(self.send_hours.start)
            && at.hour() < u32::from(self.send_hours.end)
    }

    pub fn earliest_next_send(&self, last_sent: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut candidate = last_sent + Duration::days(i64::from(self.min_days_between));
        for _ in 0..SEARCH_HORIZON_HOURS {
            if self.is_send_window(candidate) {
                return Some(candidate);
            }
            candidate += Duration::hours(1);
        }
        None
    }
}

pub fn registry() -> Vec<Cadence> {
    vec![Cadence::standard(), Cadence::gentle()]
}

pub fn by_name(name: &str) -> Option<Cadence> {
    match name {
        "standard" => Some(Cadence::standard()),
        "gentle" => Some(Cadence::gentle()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
    }

    #[test]
    fn tuesday_morning_is_in_window() {
        let c = Cadence::standard();
        let tue_9am = at(2026, 8, 18, 9);
        assert_eq!(tue_9am.weekday(), Weekday::Tue);
        assert!(c.is_send_window(tue_9am));
    }

    #[test]
    fn weekend_evening_and_upper_bound_are_outside_window() {
        let c = Cadence::standard();
        assert!(!c.is_send_window(at(2026, 8, 22, 10)));
        assert!(!c.is_send_window(at(2026, 8, 18, 20)));
        assert!(!c.is_send_window(at(2026, 8, 18, 16)));
        assert!(c.is_send_window(at(2026, 8, 18, 15)));
    }

    #[test]
    fn next_send_lands_on_the_earliest_valid_hour() {
        let c = Cadence::standard();
        assert_eq!(
            c.earliest_next_send(at(2026, 8, 21, 17)),
            Some(at(2026, 8, 25, 8))
        );
        assert_eq!(
            c.earliest_next_send(at(2026, 8, 23, 17)),
            Some(at(2026, 8, 27, 8))
        );
        assert_eq!(c.earliest_next_send(at(2026, 8, 18, 9)), Some(at(2026, 8, 25, 8)));
    }

    #[test]
    fn gentle_cadence_respects_its_longer_gap() {
        let c = Cadence::gentle();
        let next = c.earliest_next_send(at(2026, 8, 18, 9)).unwrap();
        assert!(next >= at(2026, 8, 18, 9) + Duration::days(5));
        assert!(c.is_send_window(next));
    }

    #[test]
    fn unsatisfiable_cadence_returns_none() {
        let mut c = Cadence::standard();
        c.send_days = vec![];
        assert_eq!(c.earliest_next_send(at(2026, 8, 18, 9)), None);
    }

    #[test]
    fn validate_rejects_bad_configs() {
        let mut c = Cadence::standard();
        assert!(c.validate().is_ok());
        c.send_days = vec![];
        assert!(matches!(c.validate(), Err(CadenceError::NoSendDays(_))));
        let mut c = Cadence::standard();
        c.send_hours = SendHours { start: 16, end: 8 };
        assert!(matches!(c.validate(), Err(CadenceError::InvalidHours { .. })));
        let mut c = Cadence::standard();
        c.send_hours = SendHours { start: 8, end: 30 };
        assert!(c.validate().is_err());
    }

    #[test]
    fn registry_entries_all_validate() {
        for c in registry() {
            assert!(c.validate().is_ok(), "cadence {}", c.name);
        }
    }

    #[test]
    fn registry_lookup_by_name() {
        assert_eq!(by_name("gentle").unwrap().max_steps, 2);
        assert!(by_name("nope").is_none());
    }
}
