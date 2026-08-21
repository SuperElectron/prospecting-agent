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

#[derive(Debug, Clone, PartialEq)]
pub struct GmailConfig {
    pub client_id: String,
    pub client_secret: Secret,
    pub refresh_token: Secret,
    pub senders: Vec<String>,
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
                base_url: optional(env, "MEMORY_URL")
                    .or_else(|| optional(env, "MEM0_URL"))
                    .ok_or(ConfigError::Missing("MEMORY_URL"))?,
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
    let Some(client_id) = optional(env, "GMAIL_CLIENT_ID") else {
        if is_selected {
            return Err(ConfigError::ProviderCredentials {
                provider: "gmail",
                missing: "GMAIL_CLIENT_ID",
            });
        }
        return Ok(None);
    };
    let config = GmailConfig {
        client_id,
        client_secret: get_or(env, "GMAIL_CLIENT_SECRET", "").into(),
        refresh_token: get_or(env, "GMAIL_REFRESH_TOKEN", "").into(),
        senders: split_csv(&get_or(env, "GMAIL_SENDERS", "")),
    };
    if is_selected {
        if config.client_secret.is_empty() {
            return Err(ConfigError::ProviderCredentials {
                provider: "gmail",
                missing: "GMAIL_CLIENT_SECRET",
            });
        }
        if config.refresh_token.is_empty() {
            return Err(ConfigError::ProviderCredentials {
                provider: "gmail",
                missing: "GMAIL_REFRESH_TOKEN",
            });
        }
        if config.senders.is_empty() {
            return Err(ConfigError::ProviderCredentials {
                provider: "gmail",
                missing: "GMAIL_SENDERS",
            });
        }
    }
    Ok(Some(config))
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

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
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
            ("GMAIL_CLIENT_ID", "cid"),
            ("GMAIL_CLIENT_SECRET", "csec"),
            ("GMAIL_REFRESH_TOKEN", "rtok"),
            ("GMAIL_SENDERS", "a@x.io, b@x.io"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn minimal_env_parses_with_defaults() {
        let cfg = AppConfig::from_map(&base_env()).unwrap();
        assert_eq!(cfg.email.provider, EmailProvider::Gmail);
        assert!(cfg.dry_run);
        assert_eq!(cfg.llm.api_key.expose(), "local");
        assert_eq!(cfg.email.gmail.unwrap().senders, vec!["a@x.io", "b@x.io"]);
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
    fn gmail_provider_requires_every_credential() {
        for missing in ["GMAIL_CLIENT_SECRET", "GMAIL_REFRESH_TOKEN", "GMAIL_SENDERS"] {
            let mut env = base_env();
            env.remove(missing);
            let err = AppConfig::from_map(&env).unwrap_err();
            assert_eq!(
                err,
                ConfigError::ProviderCredentials {
                    provider: "gmail",
                    missing
                }
            );
        }
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
        env.remove("GMAIL_CLIENT_SECRET");
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
        assert!(!dump.contains("csec"));
        assert!(!dump.contains("rtok"));
        assert!(dump.contains("Secret(***)"));
    }
}
