use clap::{Parser, Subcommand};
use prospecting_agent::cli::{GmailAuthArgs, run_gmail_auth};
use prospecting_agent::config::AppConfig;
use prospecting_agent::connectors::gmail::{OauthClient, load_senders};
use prospecting_agent::connectors::health::{CheckStatus, DbProbe, HealthInputs, run_health_check};
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
    SyncCsv {
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    Serve {
        #[arg(long, default_value_t = 8088)]
        port: u16,
    },
    Worker,
    Job {
        name: String,
        #[arg(long)]
        enqueue: bool,
    },
}

#[tokio::main]
async fn main() {
    observability::init_tracing("info");
    let args = Args::parse();
    let result = match args.command {
        Command::GmailAuth { daily_limit } => gmail_auth(daily_limit).await,
        Command::Health => health().await,
        Command::SyncCsv { dir } => sync_csv(dir).await,
        Command::Serve { port } => serve(port).await,
        Command::Worker => worker().await,
        Command::Job { name, enqueue } => job(&name, enqueue).await,
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
    let pool = db::connect(config.database_url.expose()).await;
    let db_probe = match &pool {
        Ok(pool) => DbProbe::Pool(pool),
        Err(e) => DbProbe::Unavailable(e.to_string()),
    };
    let gmail_config = config.email.gmail.as_ref();
    let oauth = gmail_config.and_then(|g| OauthClient::from_file(&g.client_file).ok());
    let senders = gmail_config
        .and_then(|g| load_senders(&g.senders_file).ok())
        .unwrap_or_default();
    let report = run_health_check(&HealthInputs {
        db: db_probe,
        llm_base_url: Some(&config.llm.base_url),
        memory_base_url: Some(&config.memory.base_url),
        gmail: oauth.as_ref(),
        senders: &senders,
    })
    .await;
    let rendered = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    println!("{rendered}");
    if report.status == CheckStatus::Error {
        return Err("one or more health checks failed".into());
    }
    Ok(())
}

async fn sync_csv(dir: Option<std::path::PathBuf>) -> Result<(), String> {
    let config = AppConfig::from_env().map_err(|e| e.to_string())?;
    let pool = db::connect(config.database_url.expose())
        .await
        .map_err(|e| e.to_string())?;
    db::migrate(&pool).await.map_err(|e| e.to_string())?;
    let memory = prospecting_agent::memory::MemoryClient::new(&config.memory);
    let dir = dir.unwrap_or_else(|| std::path::PathBuf::from(&config.csv_data_dir));
    let report = prospecting_agent::workflows::sync::sync_dir(&pool, &memory, &dir)
        .await
        .map_err(|e| e.to_string())?;
    let rendered = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    println!("{rendered}");
    if let Some(fatal) = report.fatal {
        return Err(format!("import stopped early: {fatal}"));
    }
    if !report.skipped.is_empty() || report.skipped_truncated > 0 {
        tracing::warn!(
            skipped = report.skipped.len() as u64 + report.skipped_truncated,
            "some rows were skipped"
        );
    }
    Ok(())
}

async fn job_context() -> Result<prospecting_agent::jobs::JobContext, String> {
    let config = AppConfig::from_env().map_err(|e| e.to_string())?;
    let pool = db::connect(config.database_url.expose())
        .await
        .map_err(|e| e.to_string())?;
    db::migrate(&pool).await.map_err(|e| e.to_string())?;
    Ok(prospecting_agent::jobs::JobContext::from_config(pool, config))
}

async fn serve(port: u16) -> Result<(), String> {
    let ctx = job_context().await?;
    let state = std::sync::Arc::new(prospecting_agent::http::AppState {
        ctx,
        webhooks: prospecting_agent::http::webhooks::WebhookRegistry::default(),
    });
    prospecting_agent::http::serve(state, port)
        .await
        .map_err(|e| e.to_string())
}

async fn worker() -> Result<(), String> {
    let ctx = job_context().await?;
    prospecting_agent::jobs::runtime::run_worker(ctx)
        .await
        .map_err(|e| e.to_string())
}

async fn job(name: &str, enqueue: bool) -> Result<(), String> {
    let ctx = job_context().await?;
    if enqueue {
        prospecting_agent::jobs::runtime::enqueue(ctx.config.database_url.expose(), name)
            .await
            .map_err(|e| e.to_string())?;
        println!("enqueued {name}");
        return Ok(());
    }
    let report = prospecting_agent::jobs::run_job(&ctx, name)
        .await
        .map_err(|e| e.to_string())?;
    let rendered = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    println!("{rendered}");
    Ok(())
}

fn gmail_config() -> Result<prospecting_agent::config::GmailConfig, String> {
    let config = AppConfig::from_env().map_err(|e| e.to_string())?;
    config
        .email
        .gmail
        .ok_or_else(|| "gmail is not the configured email provider".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_internally_consistent() {
        Args::command().debug_assert();
    }
}
