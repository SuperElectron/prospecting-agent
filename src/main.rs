use clap::{Parser, Subcommand};
use prospecting_agent::cli::{GmailAuthArgs, run_gmail_auth};
use prospecting_agent::config::AppConfig;
use prospecting_agent::connectors::gmail::load_senders;
use prospecting_agent::connectors::health::{HealthInputs, run_health_check};
use prospecting_agent::{db, observability};

#[derive(Parser)]
#[command(
    name = "prospecting-agent",
    version,
    about = "local-first AI prospecting agent"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    GmailAuth {
        #[arg(long, default_value_t = 50)]
        daily_limit: i32,
    },
    Health,
}

#[tokio::main]
async fn main() {
    observability::init_tracing("info");
    let args = Args::parse();
    let result = match args.command {
        Command::GmailAuth { daily_limit } => gmail_auth(daily_limit).await,
        Command::Health => health().await,
    };
    if let Err(message) = result {
        tracing::error!("{message}");
        std::process::exit(1);
    }
}

async fn gmail_auth(daily_limit: i32) -> Result<(), String> {
    let gmail = gmail_config()?;
    run_gmail_auth(&GmailAuthArgs {
        client_file: gmail.client_file,
        senders_file: gmail.senders_file,
        port: gmail.auth_port,
        daily_limit,
    })
    .await
    .map_err(|e| e.to_string())
}

async fn health() -> Result<(), String> {
    let config = AppConfig::from_env().map_err(|e| e.to_string())?;
    let pool = db::connect(config.database_url.expose()).await.ok();
    let senders = config
        .email
        .gmail
        .as_ref()
        .and_then(|g| load_senders(&g.senders_file).ok())
        .unwrap_or_default();
    let report = run_health_check(&HealthInputs {
        pool: pool.as_ref(),
        llm_base_url: Some(&config.llm.base_url),
        memory_base_url: Some(&config.memory.base_url),
        senders: &senders,
    })
    .await;
    let rendered = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    println!("{rendered}");
    Ok(())
}

fn gmail_config() -> Result<prospecting_agent::config::GmailConfig, String> {
    let env: prospecting_agent::config::EnvMap = std::env::vars().collect();
    AppConfig::from_map(&env)
        .ok()
        .and_then(|c| c.email.gmail)
        .map_or_else(
            || {
                Ok(prospecting_agent::config::GmailConfig {
                    client_file: std::env::var("GMAIL_CLIENT_FILE")
                        .unwrap_or_else(|_| ".claude/secrets/gmail-oauth-client.json".into()),
                    senders_file: std::env::var("GMAIL_SENDERS_FILE")
                        .unwrap_or_else(|_| ".claude/secrets/gmail-senders.json".into()),
                    auth_port: 3847,
                })
            },
            Ok,
        )
}
