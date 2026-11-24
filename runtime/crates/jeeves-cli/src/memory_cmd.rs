//! `jeeves memory` — local maintenance commands for memory.
//!
//! `jeeves memory backfill` rebuilds ledger rows from chat history; `jeeves memory rebuild` regenerates
//! the plaintext library from the facts, after folding any hand edits of its pages back in.

use anyhow::Result;
use jeeves_state::Facts;

pub async fn run(settings: &jeeves_config::Settings, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("backfill") => backfill(settings).await,
        None | Some("rebuild") => rebuild(settings).await,
        Some(other) => {
            anyhow::bail!("Unknown subcommand '{other}'. Options: rebuild (default), backfill.")
        }
    }
}

/// Turn what `chat_log` already holds into ledger rows, marked `folded`.
///
/// Only useful once (after migration 0020) and honest about its ceiling: the view it
/// reads was already truncated to 200 characters per tool output, so those turns cannot
/// be rebuilt exactly — the `folded` mark says so on every row.
async fn backfill(settings: &jeeves_config::Settings) -> Result<()> {
    let pool = jeeves_state::open(&settings.home.join("state.db")).await?;
    let before = jeeves_state::Ledger::new(pool.clone()).count().await?;
    if before > 0 {
        println!("The ledger already holds {before} turns; not backfilling (it only runs once).");
        return Ok(());
    }
    println!(
        "Backfilling the ledger from chat_log (historical tool output survives as summaries only, marked folded)…"
    );
    let report = jeeves_state::ledger::backfill_from_chat(&pool).await?;
    println!(
        "Backfill done: {} sessions, {} turns, {} rows. Those turns can only be rebuilt down to the folded text — there was never a fuller record.",
        report.sessions, report.turns, report.rows
    );
    Ok(())
}

/// Regenerate the plaintext library from the facts (`<home>/memory/`).
///
/// Not a migration: a maintenance command for the day a page was deleted, a fact was
/// edited behind the file's back, or someone wants to see the library rebuilt from the
/// index. Pages a human edited are left alone (and counted), because the file wins.
async fn rebuild(settings: &jeeves_config::Settings) -> Result<()> {
    let pool = jeeves_state::open(&settings.home.join("state.db")).await?;
    let facts = Facts::new(pool);
    let bundle = jeeves_memory::bundle::Bundle::new(&settings.home);
    let restored = jeeves_memory::mirror::reindex_from_pages(&facts, &bundle).await?;
    if restored > 0 {
        println!("Restored {restored} facts from their pages (same ids as before)");
    }
    let sync = jeeves_memory::mirror::sync_edited_pages(&facts, &bundle).await?;
    if sync.changed() {
        println!(
            "Took your page edits first: {} facts changed, {} removed, {} added",
            sync.updated, sync.deleted, sync.added
        );
    }
    let report = jeeves_memory::mirror::mirror_facts(&facts, &bundle).await?;
    println!(
        "Memory library: {} pages written, {} pages left alone (edited by hand), {} subjects have no facts left (pages kept)",
        report.written, report.skipped, report.orphaned
    );
    println!(
        "Catalog and change history are in {}",
        bundle.root().join("index.md").display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn unknown_subcommand_is_an_error() {
        let settings = jeeves_config::Settings::default();
        let err = super::run(&settings, &["bogus".to_string()])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Unknown subcommand 'bogus'"));
    }
}
