#[derive(Debug, Clone, PartialEq)]
pub struct PreflightConfig {
    pub carpet_bomb_window_days: i32,
    pub max_contacts_per_window: i64,
    pub negative_event_delay_days: i64,
    pub warm_intro_cadence: String,
    pub warm_intro_max_emails: u8,
}

impl Default for PreflightConfig {
    fn default() -> Self {
        Self {
            carpet_bomb_window_days: 7,
            max_contacts_per_window: 2,
            negative_event_delay_days: 21,
            warm_intro_cadence: "gentle".into(),
            warm_intro_max_emails: 2,
        }
    }
}
