//! Schema migrations keyed by `PRAGMA user_version`. Append, never edit.

use std::time::Duration;

use rusqlite::{Connection, ErrorCode, TransactionBehavior};

const MIGRATIONS: &[&str] = &[
    // v1
    "CREATE TABLE meta (
         key   TEXT PRIMARY KEY,
         value BLOB NOT NULL
     );
     CREATE TABLE ops (
         author      BLOB    NOT NULL,
         seq         INTEGER NOT NULL,
         class       INTEGER NOT NULL,
         header      BLOB    NOT NULL,
         sig         BLOB    NOT NULL,
         body        BLOB    NOT NULL,
         received_ms INTEGER NOT NULL,
         PRIMARY KEY (author, seq)
     ) WITHOUT ROWID;
     CREATE INDEX ops_class ON ops (class);
     CREATE TABLE peers (
         device       BLOB PRIMARY KEY,
         addrs        TEXT NOT NULL DEFAULT '[]',
         last_seen_ms INTEGER,
         last_sync_ms INTEGER,
         last_error   TEXT
     );
     CREATE TABLE manual_peers (
         addr         TEXT PRIMARY KEY,
         allow_public INTEGER NOT NULL DEFAULT 0
     );",
];

/// Runs `f` again for a short while if SQLite reports the database busy. Several openers can
/// set up a brand-new file at once (the UI and the Android background worker), and SQLite
/// answers some of those collisions with an immediate "locked" instead of waiting.
pub(crate) fn retry_busy<T>(mut f: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let mut attempts = 0;
    loop {
        match f() {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if matches!(e.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                    && attempts < 200 =>
            {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

fn version(conn: &Connection) -> rusqlite::Result<usize> {
    conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
        .map(|v| v as usize)
}

pub fn run(conn: &mut Connection) -> rusqlite::Result<()> {
    if retry_busy(|| version(conn))? >= MIGRATIONS.len() {
        return Ok(());
    }
    retry_busy(|| {
        // IMMEDIATE takes the write lock first, so a second opener waits here and then finds
        // the schema already created instead of failing with "table already exists".
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let done = version(&tx)?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(done) {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        }
        tx.commit()
    })
}
