pub const SWEEP_LOCK_KEY: i64 = 73_461_122;

pub async fn cross_process_sweep_lock() -> sqlx::PgConnection {
    use sqlx::Connection;
    let url = std::env::var("TEST_DATABASE_URL").expect("guard runs only with a test database");
    let mut conn = sqlx::PgConnection::connect(&url).await.expect("lock connection");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SWEEP_LOCK_KEY)
        .execute(&mut conn)
        .await
        .expect("advisory lock");
    conn
}
