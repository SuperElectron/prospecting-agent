use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing_subscriber::EnvFilter;

pub fn init_tracing(default_level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

#[derive(Default)]
pub struct Counters {
    inner: Mutex<HashMap<&'static str, AtomicU64>>,
}

impl Counters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn incr(&self, name: &'static str) {
        self.add(name, 1);
    }

    pub fn add(&self, name: &'static str, amount: u64) {
        let mut map = self.inner.lock().expect("counters lock poisoned");
        map.entry(name)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(amount, Ordering::Relaxed);
    }

    pub fn get(&self, name: &'static str) -> u64 {
        let map = self.inner.lock().expect("counters lock poisoned");
        map.get(name).map_or(0, |c| c.load(Ordering::Relaxed))
    }

    pub fn snapshot(&self) -> HashMap<&'static str, u64> {
        let map = self.inner.lock().expect("counters lock poisoned");
        map.iter().map(|(k, v)| (*k, v.load(Ordering::Relaxed))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_and_snapshot() {
        let c = Counters::new();
        c.incr("emails_sent");
        c.add("emails_sent", 4);
        c.incr("replies");
        assert_eq!(c.get("emails_sent"), 5);
        assert_eq!(c.get("replies"), 1);
        assert_eq!(c.get("unknown"), 0);
        let snap = c.snapshot();
        assert_eq!(snap.get("emails_sent"), Some(&5));
    }
}
