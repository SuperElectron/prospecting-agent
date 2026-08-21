use crate::config::cadence::Cadence;
use crate::config::outreach::PreflightConfig;
use crate::config::targeting::{DiscoveryBudget, IcpCriteria};
use crate::domain::Seniority;
use std::collections::HashMap;

use super::secret::Secret;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("missing required env var {0}")]
    Missing(&'static str),
    #[error("invalid value for {key}: {reason}")]
    Invalid { key: &'static str, reason: String },
    #[error("email provider {provider} selected but {missing} is not set")]
    ProviderCredentials {
        provider: &'static str,
        missing: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub database_url: Secret,
    pub llm: LlmConfig,
    pub memory: MemoryConfig,
    pub apollo_api_key: Secret,
    pub tavily_api_key: Secret,
    pub email: EmailConfig,
    pub hubspot: Option<HubspotConfig>,
    pub linkedin: Option<LinkedinConfig>,
    pub slack_webhook_url: Option<Secret>,
    pub log_level: String,
    pub dry_run: bool,
    pub csv_data_dir: String,
    pub targeting: IcpCriteria,
    pub discovery: DiscoveryBudget,
    pub preflight: PreflightConfig,
    pub apollo_daily_credit_cap: u16,
    pub cadence: Cadence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryConfig {
    pub base_url: String,
    pub user: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: Secret,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmailConfig {
    pub provider: EmailProvider,
    pub gmail: Option<GmailConfig>,
    pub sendgrid_api_key: Option<Secret>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailProvider {
    Gmail,
    Sendgrid,
}

pub const GMAIL_CLIENT_FILE_DEFAULT: &str = ".claude/secrets/gmail-oauth-client.json";
pub const GMAIL_SENDERS_FILE_DEFAULT: &str = ".claude/secrets/gmail-senders.json";

#[derive(Debug, Clone, PartialEq)]
pub struct GmailConfig {
    pub client_file: String,
    pub senders_file: String,
    pub auth_port: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HubspotConfig {
    pub access_token: Secret,
    pub owner_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkedinConfig {
    pub heyreach_api_key: Secret,
    pub heyreach_campaign_id: Option<String>,
    pub daily_limit: u16,
}

pub type EnvMap = HashMap<String, String>;

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_map(&std::env::vars().collect())
    }

    pub fn from_map(env: &EnvMap) -> Result<Self, ConfigError> {
        let email = EmailConfig::from_map(env)?;
        Ok(Self {
            database_url: required(env, "DATABASE_URL")?.into(),
            llm: LlmConfig {
                base_url: required(env, "LLM_BASE_URL")?,
                api_key: get_or(env, "LLM_API_KEY", "local").into(),
                model: required(env, "LLM_MODEL")?,
            },
            memory: MemoryConfig {
                base_url: required(env, "MEMORY_URL")?,
                user: get_or(env, "MEMORY_USER", "prospecting"),
            },
            apollo_api_key: required(env, "APOLLO_API_KEY")?.into(),
            tavily_api_key: required(env, "TAVILY_API_KEY")?.into(),
            email,
            hubspot: optional(env, "HUBSPOT_ACCESS_TOKEN").map(|token| HubspotConfig {
                access_token: token.into(),
                owner_id: optional(env, "HUBSPOT_OWNER_ID"),
            }),
            linkedin: linkedin_from_map(env)?,
            slack_webhook_url: optional(env, "SLACK_WEBHOOK_URL").map(Secret::new),
            log_level: get_or(env, "LOG_LEVEL", "info"),
            dry_run: parse_bool(env, "DRY_RUN", true)?,
            csv_data_dir: get_or(env, "CSV_DATA_DIR", "./data"),
            targeting: targeting_from_map(env)?,
            discovery: DiscoveryBudget {
                contacts_per_account: parse_number(env, "DISCOVERY_CONTACTS_PER_ACCOUNT", 3)?,
                max_credits_per_run: parse_number(env, "DISCOVERY_CREDITS_PER_RUN", 15)?,
            },
            preflight: PreflightConfig {
                carpet_bomb_window_days: parse_number(env, "CARPET_BOMB_WINDOW_DAYS", 7)?,
                max_contacts_per_window: parse_number(env, "MAX_CONTACTS_PER_WINDOW", 2)?,
                negative_event_delay_days: parse_number(env, "NEGATIVE_EVENT_DELAY_DAYS", 21)?,
                warm_intro_cadence: get_or(env, "WARM_INTRO_CADENCE", "gentle"),
                warm_intro_max_emails: parse_number(env, "WARM_INTRO_MAX_EMAILS", 2)?,
            },
            apollo_daily_credit_cap: parse_number(env, "APOLLO_DAILY_CREDIT_CAP", 50)?,
            cadence: cadence_from_map(env)?,
        })
    }
}

impl EmailConfig {
    fn from_map(env: &EnvMap) -> Result<Self, ConfigError> {
        let provider = match get_or(env, "EMAIL_PROVIDER", "gmail").as_str() {
            "gmail" => EmailProvider::Gmail,
            "sendgrid" => EmailProvider::Sendgrid,
            other => {
                return Err(ConfigError::Invalid {
                    key: "EMAIL_PROVIDER",
                    reason: format!("unknown provider '{other}'"),
                });
            }
        };
        let gmail = gmail_from_map(env, provider == EmailProvider::Gmail)?;
        let sendgrid_api_key = optional(env, "SENDGRID_API_KEY").map(Secret::new);
        if provider == EmailProvider::Sendgrid && sendgrid_api_key.is_none() {
            return Err(ConfigError::ProviderCredentials {
                provider: "sendgrid",
                missing: "SENDGRID_API_KEY",
            });
        }
        Ok(Self {
            provider,
            gmail,
            sendgrid_api_key,
        })
    }
}

fn gmail_from_map(env: &EnvMap, is_selected: bool) -> Result<Option<GmailConfig>, ConfigError> {
    let client_file = optional(env, "GMAIL_CLIENT_FILE");
    if client_file.is_none() && !is_selected {
        return Ok(None);
    }
    let auth_port = get_or(env, "GMAIL_AUTH_PORT", "3847")
        .parse()
        .map_err(|_| ConfigError::Invalid {
            key: "GMAIL_AUTH_PORT",
            reason: "must be a port number".into(),
        })?;
    Ok(Some(GmailConfig {
        client_file: client_file.unwrap_or_else(|| GMAIL_CLIENT_FILE_DEFAULT.into()),
        senders_file: get_or(env, "GMAIL_SENDERS_FILE", GMAIL_SENDERS_FILE_DEFAULT),
        auth_port,
    }))
}

fn linkedin_from_map(env: &EnvMap) -> Result<Option<LinkedinConfig>, ConfigError> {
    if !parse_bool(env, "LINKEDIN_ENABLED", false)? {
        return Ok(None);
    }
    let heyreach_api_key = required(env, "HEYREACH_API_KEY")?.into();
    Ok(Some(LinkedinConfig {
        heyreach_api_key,
        heyreach_campaign_id: optional(env, "HEYREACH_CAMPAIGN_ID"),
        daily_limit: parse_u16(env, "LINKEDIN_DAILY_LIMIT", 20)?,
    }))
}

fn cadence_from_map(env: &EnvMap) -> Result<Cadence, ConfigError> {
    let name = get_or(env, "CADENCE", "standard");
    let mut cadence = crate::config::cadence::by_name(&name).ok_or(ConfigError::Invalid {
        key: "CADENCE",
        reason: format!("no cadence named {name}"),
    })?;
    cadence.max_steps = parse_number(env, "CADENCE_MAX_STEPS", cadence.max_steps)?;
    cadence.min_days_between = parse_number(env, "CADENCE_MIN_DAYS_BETWEEN", cadence.min_days_between)?;
    cadence.send_hours.start = parse_number(env, "SEND_HOURS_START", cadence.send_hours.start)?;
    cadence.send_hours.end = parse_number(env, "SEND_HOURS_END", cadence.send_hours.end)?;
    if let Some(raw) = optional(env, "SEND_DAYS") {
        let days: Result<Vec<chrono::Weekday>, _> = raw
            .split(',')
            .map(str::trim)
            .filter(|day| !day.is_empty())
            .map(str::parse)
            .collect();
        cadence.send_days = days.map_err(|_| ConfigError::Invalid {
            key: "SEND_DAYS",
            reason: format!("cannot parse {raw} as weekday names"),
        })?;
    }
    cadence.validate().map_err(|e| ConfigError::Invalid {
        key: "SEND_HOURS_START",
        reason: e.to_string(),
    })?;
    Ok(cadence)
}

fn targeting_from_map(env: &EnvMap) -> Result<IcpCriteria, ConfigError> {
    let mut criteria = IcpCriteria::default();
    if let Some(titles) = optional(env, "TARGET_TITLES") {
        let parsed: Vec<String> = titles
            .split(',')
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
            .collect();
        if parsed.is_empty() {
            return Err(ConfigError::Invalid {
                key: "TARGET_TITLES",
                reason: "no titles after parsing".into(),
            });
        }
        criteria.target_titles = parsed;
    }
    if let Some(raw) = optional(env, "MIN_SENIORITY") {
        criteria.min_seniority = serde_json::from_value::<Seniority>(serde_json::Value::String(raw.clone()))
            .map_err(|_| ConfigError::Invalid {
                key: "MIN_SENIORITY",
                reason: format!("unknown seniority {raw}"),
            })?;
    }
    if let Some(domains) = optional(env, "DISQUALIFIED_DOMAINS") {
        criteria.disqualified_domains = domains
            .split(',')
            .map(str::trim)
            .filter(|domain| !domain.is_empty())
            .map(crate::domain::normalize_domain)
            .collect();
    }
    Ok(criteria)
}

fn parse_number<T: std::str::FromStr>(env: &EnvMap, key: &'static str, default: T) -> Result<T, ConfigError> {
    match optional(env, key) {
        Some(raw) => raw.trim().parse().map_err(|_| ConfigError::Invalid {
            key,
            reason: format!("cannot parse {raw} as a number"),
        }),
        None => Ok(default),
    }
}

fn required(env: &EnvMap, key: &'static str) -> Result<String, ConfigError> {
    optional(env, key).ok_or(ConfigError::Missing(key))
}

fn optional(env: &EnvMap, key: &str) -> Option<String> {
    env.get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn get_or(env: &EnvMap, key: &str, default: &str) -> String {
    optional(env, key).unwrap_or_else(|| default.to_string())
}

fn parse_bool(env: &EnvMap, key: &'static str, default: bool) -> Result<bool, ConfigError> {
    match optional(env, key) {
        None => Ok(default),
        Some(v) => match v.to_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            other => Err(ConfigError::Invalid {
                key,
                reason: format!("expected boolean, got '{other}'"),
            }),
        },
    }
}

fn parse_u16(env: &EnvMap, key: &'static str, default: u16) -> Result<u16, ConfigError> {
    match optional(env, key) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| ConfigError::Invalid {
            key,
            reason: format!("expected integer, got '{v}'"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_env() -> EnvMap {
        [
            ("DATABASE_URL", "postgres://localhost/p"),
            ("LLM_BASE_URL", "http://localhost:8000/v1"),
            ("LLM_MODEL", "gpt-oss-120b"),
            ("MEMORY_URL", "http://localhost:8765"),
            ("APOLLO_API_KEY", "ap-key"),
            ("TAVILY_API_KEY", "tv-key"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn cadence_parses_selects_overrides_and_validates() {
        let mut env = base_env();
        let cfg = AppConfig::from_map(&env).unwrap();
        assert_eq!(cfg.cadence, Cadence::standard());

        env.insert("CADENCE".into(), "gentle".into());
        env.insert("CADENCE_MAX_STEPS".into(), "4".into());
        env.insert("SEND_HOURS_START".into(), "9".into());
        env.insert("SEND_DAYS".into(), "mon, fri".into());
        let cfg = AppConfig::from_map(&env).unwrap();
        assert_eq!(cfg.cadence.name, "gentle");
        assert_eq!(cfg.cadence.max_steps, 4);
        assert_eq!(cfg.cadence.send_hours.start, 9);
        assert_eq!(
            cfg.cadence.send_days,
            vec![chrono::Weekday::Mon, chrono::Weekday::Fri]
        );

        env.insert("CADENCE".into(), "aggressive".into());
        assert!(matches!(
            AppConfig::from_map(&env),
            Err(ConfigError::Invalid { key: "CADENCE", .. })
        ));
        env.insert("CADENCE".into(), "gentle".into());
        env.insert("SEND_HOURS_END".into(), "9".into());
        assert!(AppConfig::from_map(&env).is_err());
        env.insert("SEND_HOURS_END".into(), "16".into());
        env.insert("SEND_DAYS".into(), "funday".into());
        assert!(matches!(
            AppConfig::from_map(&env),
            Err(ConfigError::Invalid { key: "SEND_DAYS", .. })
        ));
    }

    #[test]
    fn targeting_and_spend_parse_with_overrides_and_defaults() {
        let mut env = base_env();
        let cfg = AppConfig::from_map(&env).unwrap();
        assert_eq!(cfg.targeting, IcpCriteria::default());
        assert_eq!(cfg.discovery, DiscoveryBudget::default());
        assert_eq!(cfg.preflight, PreflightConfig::default());
        assert_eq!(cfg.apollo_daily_credit_cap, 50);

        env.insert("TARGET_TITLES".into(), " CTO , VP Engineering ".into());
        env.insert("MIN_SENIORITY".into(), "vp".into());
        env.insert("DISQUALIFIED_DOMAINS".into(), "WWW.Bad.com, rival.io".into());
        env.insert("DISCOVERY_CREDITS_PER_RUN".into(), "5".into());
        env.insert("APOLLO_DAILY_CREDIT_CAP".into(), "20".into());
        let cfg = AppConfig::from_map(&env).unwrap();
        assert_eq!(cfg.targeting.target_titles, vec!["CTO", "VP Engineering"]);
        assert_eq!(cfg.targeting.min_seniority, Seniority::Vp);
        assert_eq!(cfg.targeting.disqualified_domains, vec!["bad.com", "rival.io"]);
        assert_eq!(cfg.discovery.max_credits_per_run, 5);
        assert_eq!(cfg.apollo_daily_credit_cap, 20);

        env.insert("MIN_SENIORITY".into(), "boss".into());
        assert!(matches!(
            AppConfig::from_map(&env),
            Err(ConfigError::Invalid {
                key: "MIN_SENIORITY",
                ..
            })
        ));
        env.insert("MIN_SENIORITY".into(), "vp".into());
        env.insert("TARGET_TITLES".into(), " , ".into());
        assert!(matches!(
            AppConfig::from_map(&env),
            Err(ConfigError::Invalid {
                key: "TARGET_TITLES",
                ..
            })
        ));
    }

    #[test]
    fn minimal_env_parses_with_defaults() {
        let cfg = AppConfig::from_map(&base_env()).unwrap();
        assert_eq!(cfg.email.provider, EmailProvider::Gmail);
        assert!(cfg.dry_run);
        assert_eq!(cfg.llm.api_key.expose(), "local");
        let gmail = cfg.email.gmail.unwrap();
        assert_eq!(gmail.client_file, GMAIL_CLIENT_FILE_DEFAULT);
        assert_eq!(gmail.senders_file, GMAIL_SENDERS_FILE_DEFAULT);
        assert_eq!(gmail.auth_port, 3847);
        assert!(cfg.linkedin.is_none());
        assert!(cfg.hubspot.is_none());
    }

    #[test]
    fn missing_required_key_names_the_key() {
        let mut env = base_env();
        env.remove("APOLLO_API_KEY");
        assert_eq!(
            AppConfig::from_map(&env).unwrap_err(),
            ConfigError::Missing("APOLLO_API_KEY")
        );
    }

    #[test]
    fn empty_string_counts_as_missing() {
        let mut env = base_env();
        env.insert("TAVILY_API_KEY".into(), "  ".into());
        assert_eq!(
            AppConfig::from_map(&env).unwrap_err(),
            ConfigError::Missing("TAVILY_API_KEY")
        );
    }

    #[test]
    fn gmail_file_paths_and_port_are_overridable() {
        let mut env = base_env();
        env.insert("GMAIL_CLIENT_FILE".into(), "/tmp/client.json".into());
        env.insert("GMAIL_SENDERS_FILE".into(), "/tmp/senders.json".into());
        env.insert("GMAIL_AUTH_PORT".into(), "4000".into());
        let gmail = AppConfig::from_map(&env).unwrap().email.gmail.unwrap();
        assert_eq!(gmail.client_file, "/tmp/client.json");
        assert_eq!(gmail.senders_file, "/tmp/senders.json");
        assert_eq!(gmail.auth_port, 4000);
    }

    #[test]
    fn bad_gmail_auth_port_is_rejected() {
        let mut env = base_env();
        env.insert("GMAIL_AUTH_PORT".into(), "not-a-port".into());
        assert!(matches!(
            AppConfig::from_map(&env).unwrap_err(),
            ConfigError::Invalid {
                key: "GMAIL_AUTH_PORT",
                ..
            }
        ));
    }

    #[test]
    fn sendgrid_provider_requires_its_key() {
        let mut env = base_env();
        env.insert("EMAIL_PROVIDER".into(), "sendgrid".into());
        let err = AppConfig::from_map(&env).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::ProviderCredentials {
                provider: "sendgrid",
                ..
            }
        ));
        env.insert("SENDGRID_API_KEY".into(), "sg-key".into());
        let cfg = AppConfig::from_map(&env).unwrap();
        assert_eq!(cfg.email.provider, EmailProvider::Sendgrid);
    }

    #[test]
    fn sendgrid_provider_tolerates_partial_gmail_credentials() {
        let mut env = base_env();
        env.insert("EMAIL_PROVIDER".into(), "sendgrid".into());
        env.insert("SENDGRID_API_KEY".into(), "sg-key".into());
        env.remove("GMAIL_CLIENT_FILE");
        assert!(AppConfig::from_map(&env).is_ok());
    }

    #[test]
    fn linkedin_enabled_requires_heyreach_key() {
        let mut env = base_env();
        env.insert("LINKEDIN_ENABLED".into(), "true".into());
        assert_eq!(
            AppConfig::from_map(&env).unwrap_err(),
            ConfigError::Missing("HEYREACH_API_KEY")
        );
        env.insert("HEYREACH_API_KEY".into(), "hr-key".into());
        let cfg = AppConfig::from_map(&env).unwrap();
        assert_eq!(cfg.linkedin.unwrap().daily_limit, 20);
    }

    #[test]
    fn bad_bool_is_rejected() {
        let mut env = base_env();
        env.insert("DRY_RUN".into(), "maybe".into());
        assert!(matches!(
            AppConfig::from_map(&env).unwrap_err(),
            ConfigError::Invalid { key: "DRY_RUN", .. }
        ));
    }

    #[test]
    fn unknown_email_provider_is_rejected() {
        let mut env = base_env();
        env.insert("EMAIL_PROVIDER".into(), "smtp".into());
        assert!(matches!(
            AppConfig::from_map(&env).unwrap_err(),
            ConfigError::Invalid {
                key: "EMAIL_PROVIDER",
                ..
            }
        ));
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let cfg = AppConfig::from_map(&base_env()).unwrap();
        let dump = format!("{cfg:?}");
        assert!(!dump.contains("ap-key"));
        assert!(!dump.contains("tv-key"));
        assert!(!dump.contains("ap-key"));
        assert!(!dump.contains("tv-key"));
        assert!(dump.contains("Secret(***)"));
    }
}
