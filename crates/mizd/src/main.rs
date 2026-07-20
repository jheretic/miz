//! mizd — a D-Bus daemon exposing miz layered-package operations over
//! `org.archetype.miz1`.
//!
//! Phase 2: serve the Manager object and request the well-known name. No worker
//! thread, no libalpm. Uses zbus's internal async executor (via the `async-io`
//! feature); no tokio.

mod job;
mod manager;
mod sink;

use manager::Manager;

const NAME: &str = "org.archetype.miz1";
const MANAGER_PATH: &str = "/org/archetype/miz1";

fn main() -> zbus::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    zbus::block_on(run())
}

async fn run() -> zbus::Result<()> {
    // Dev/VM introspection without root: MIZD_SESSION_BUS routes to the session
    // bus. Production runs on the system bus (D-Bus activated, as root).
    let builder = if std::env::var_os("MIZD_SESSION_BUS").is_some() {
        tracing::info!("using session bus (MIZD_SESSION_BUS set)");
        zbus::connection::Builder::session()?
    } else {
        zbus::connection::Builder::system()?
    };

    let conn = builder
        .name(NAME)?
        .serve_at(MANAGER_PATH, Manager::new())?
        .build()
        .await?;

    tracing::info!("serving {NAME} at {MANAGER_PATH}");

    // Park: keep the connection alive; the async executor services requests.
    std::future::pending::<()>().await;
    drop(conn);
    Ok(())
}
