use crate::connectors::ConnectorError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    Info,
    Warning,
    Error,
}

pub trait Notifier {
    fn notify(
        &self,
        level: NotifyLevel,
        message: &str,
    ) -> impl Future<Output = Result<(), ConnectorError>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    async fn notify(&self, level: NotifyLevel, message: &str) -> Result<(), ConnectorError> {
        match level {
            NotifyLevel::Info => tracing::info!(target: "notify", "{message}"),
            NotifyLevel::Warning => tracing::warn!(target: "notify", "{message}"),
            NotifyLevel::Error => tracing::error!(target: "notify", "{message}"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn log_notifier_accepts_every_level() {
        let notifier = LogNotifier;
        for level in [NotifyLevel::Info, NotifyLevel::Warning, NotifyLevel::Error] {
            notifier.notify(level, "test message").await.unwrap();
        }
    }
}
