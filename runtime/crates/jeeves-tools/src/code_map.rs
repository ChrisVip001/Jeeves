//! Ephemeral FTS5 declaration index. Rebuild from bounded source reads on each call so
//! uncommitted edits are visible and no repository files or persistent index are written.
use anyhow::Result;
use sqlx::{Connection, SqliteConnection};
use std::path::PathBuf;

use crate::search::{
    declaration, display_path, walk_files, GREP_MAX_FILE_BYTES, GREP_MAX_LINE_CHARS,
    SEARCH_MAX_MATCHES,
};

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DECLARATIONS: usize = 30_000;
type Declaration = (String, String, i64, String);

fn collect(root: PathBuf) -> (Vec<Declaration>, bool) {
    let mut rows = Vec::new();
    let mut bytes = 0;
    let cut = walk_files(&root, &mut |path| {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ![
            "rs", "ts", "tsx", "js", "jsx", "vue", "py", "go", "java", "c", "h", "cpp",
        ]
        .contains(&ext)
        {
            return true;
        }
        let Ok(meta) = std::fs::metadata(path) else {
            return true;
        };
        if meta.len() > GREP_MAX_FILE_BYTES {
            return true;
        }
        bytes += meta.len();
        if bytes > MAX_BYTES {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return true;
        };
        let relative = display_path(&root, path);
        for (index, line) in text.lines().enumerate() {
            let Some(name) = declaration(line) else {
                continue;
            };
            rows.push((
                name.into(),
                relative.clone(),
                (index + 1) as i64,
                line.trim().chars().take(GREP_MAX_LINE_CHARS).collect(),
            ));
            if rows.len() >= MAX_DECLARATIONS {
                return false;
            }
        }
        true
    });
    (rows, cut)
}

pub(crate) async fn navigate(root: PathBuf, needle: Option<String>) -> Result<String> {
    if needle.as_deref().is_some_and(|name| {
        name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_')
    }) {
        return Ok("No declarations found.".into());
    }
    let (rows, cut) = tokio::task::spawn_blocking(move || collect(root)).await?;
    let mut db = SqliteConnection::connect("sqlite::memory:").await?;
    sqlx::query("CREATE VIRTUAL TABLE declarations USING fts5(name, path UNINDEXED, line UNINDEXED, preview UNINDEXED, tokenize='unicode61 tokenchars _')")
        .execute(&mut db).await?;
    let mut tx = db.begin().await?;
    for chunk in rows.chunks(200) {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "INSERT INTO declarations(name,path,line,preview) ",
        );
        query.push_values(chunk, |mut b, (name, path, line, preview)| {
            b.push_bind(name)
                .push_bind(path)
                .push_bind(line)
                .push_bind(preview);
        });
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let limit = (SEARCH_MAX_MATCHES + 1) as i64;
    let hits: Vec<(String, i64, String)> = if let Some(name) = needle {
        sqlx::query_as("SELECT path,line,preview FROM declarations WHERE declarations MATCH ? AND name=? ORDER BY rowid LIMIT ?")
            .bind(format!("\"{name}\"")).bind(name).bind(limit).fetch_all(&mut db).await?
    } else {
        sqlx::query_as("SELECT path,line,preview FROM declarations ORDER BY rowid LIMIT ?")
            .bind(limit)
            .fetch_all(&mut db)
            .await?
    };
    let truncated = cut || hits.len() > SEARCH_MAX_MATCHES;
    let mut out = hits
        .into_iter()
        .take(SEARCH_MAX_MATCHES)
        .map(|(p, l, s)| format!("{p}:{l}: {s}"))
        .collect::<Vec<_>>()
        .join("\n");
    if out.is_empty() {
        out = "No declarations found.".into();
    }
    if truncated {
        out.push_str("\n[Stopped at search limit; narrow the path.]\n");
    }
    Ok(out)
}
