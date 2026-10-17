//! Databases for tests: on the throwaway Postgres at `TEST_POSTGRES_URL`
//! (`make test` starts one and sets it). There's deliberately no default:
//! test data must never land in a real stack's Postgres, so one holding a
//! real stack's databases is refused too.

use sqlx::Connection;
use tokio::sync::OnceCell;
use uuid::Uuid;

/// The real stack's databases, which a test Postgres mustn't have.
const REAL: [&str; 4] = ["auth", "characters", "combats", "monsters"];

/// The test database `name`, shared by whichever tests use it.
pub async fn database_url(name: &str) -> String {
    static CHECKED: OnceCell<()> = OnceCell::const_new();
    let server = std::env::var("TEST_POSTGRES_URL").expect(
        "set TEST_POSTGRES_URL to a throwaway Postgres (`make test` starts one and sets it)",
    );
    let server = server.trim_end_matches('/').to_string();
    CHECKED.get_or_init(|| refuse_a_real_stack(&server)).await;
    format!("{server}/{name}")
}

/// A test database of its own, for a test that mustn't see any other's
/// data (`open_database` creates it).
pub async fn fresh_database_url(prefix: &str) -> String {
    database_url(&format!("{prefix}_{}", Uuid::new_v4().simple())).await
}

async fn refuse_a_real_stack(server: &str) {
    let mut connection = sqlx::PgConnection::connect(&format!("{server}/postgres"))
        .await
        .expect("TEST_POSTGRES_URL can't be reached");
    let real: Vec<(String,)> =
        sqlx::query_as("SELECT datname FROM pg_database WHERE datname = ANY($1)")
            .bind(&REAL[..])
            .fetch_all(&mut connection)
            .await
            .expect("listing the databases");
    assert!(
        real.is_empty(),
        "TEST_POSTGRES_URL is a real stack's Postgres (it holds {real:?}); \
         tests only run against a throwaway one"
    );
}
