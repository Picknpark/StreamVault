use std::path::PathBuf;
use streamvault_core::StreamVaultCore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./StreamVault"));

    let core = StreamVaultCore::initialize(Some(root))?;
    let settings = core.db.get_settings()?;
    core.db.save_settings(&settings)?;
    core.db.with_conn(|conn| {
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    })?;
    core.graceful_shutdown();

    println!(
        "Initialized portable StreamVault folder at {}",
        core.paths.root_dir.display()
    );
    Ok(())
}
