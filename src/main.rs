use prospecting_agent::observability;

fn main() {
    observability::init_tracing("info");
    tracing::info!("prospecting-agent starting");
}
