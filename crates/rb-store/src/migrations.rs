//! File-discovered, checksummed, transactional migration runner.

use crate::error::migration_err;
use include_dir::{include_dir, Dir};
use rb_types::{Error, Result};
use sha2::{Digest, Sha256};

/// Migrations embedded from `crates/rb-store/migrations` at compile time.
static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

/// A single migration: numeric version, file name, SQL body.
struct Migration {
    version: i64,
    name: String,
    sql: String,
}

/// Hex-encoded sha256 of the SQL body.
fn checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Discover `NNN_*.sql` migration files from the embedded directory,
/// ordered ascending by their numeric prefix.
fn discover() -> Result<Vec<Migration>> {
    let mut migs: Vec<Migration> = Vec::new();
    for file in MIGRATIONS_DIR.files() {
        let name = match file.path().file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.ends_with(".sql") {
            continue;
        }
        let prefix: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
        if prefix.is_empty() {
            return Err(Error::Migration(format!(
                "migration file has no numeric prefix: {name}"
            )));
        }
        let version: i64 = prefix
            .parse()
            .map_err(|_| Error::Migration(format!("invalid numeric prefix in {name}")))?;
        let sql = file
            .contents_utf8()
            .ok_or_else(|| Error::Migration(format!("migration {name} is not UTF-8")))?
            .to_string();
        migs.push(Migration { version, name, sql });
    }
    migs.sort_by_key(|m| m.version);
    Ok(migs)
}

/// Create the `_migrations` ledger if it does not already exist.
fn ensure_migrations_table(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (\n\
           version    INTEGER PRIMARY KEY,\n\
           name       TEXT NOT NULL,\n\
           checksum   TEXT NOT NULL,\n\
           applied_at INTEGER NOT NULL\n\
         );",
    )
    .map_err(migration_err)
}

/// The recorded checksum for `version`, if any.
fn recorded_checksum(conn: &rusqlite::Connection, version: i64) -> Result<Option<String>> {
    conn.query_row(
        "SELECT checksum FROM _migrations WHERE version = ?1",
        [version],
        |row| row.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(migration_err(other)),
    })
}

/// Apply every not-yet-recorded migration; verify checksums of recorded ones.
fn apply_all(conn: &rusqlite::Connection, migs: &[Migration]) -> Result<()> {
    for m in migs {
        match recorded_checksum(conn, m.version)? {
            Some(existing) => {
                let current = checksum(&m.sql);
                if existing != current {
                    return Err(Error::Migration(format!(
                        "checksum mismatch for migration {} ({}): \
                         recorded {existing}, file {current}",
                        m.version, m.name
                    )));
                }
                // Already applied and unchanged: no-op.
            }
            None => {
                let sum = checksum(&m.sql);
                conn.execute_batch("BEGIN;").map_err(migration_err)?;
                let applied = (|| -> Result<()> {
                    conn.execute_batch(&m.sql).map_err(migration_err)?;
                    conn.execute(
                        "INSERT INTO _migrations (version, name, checksum, applied_at) \
                         VALUES (?1, ?2, ?3, strftime('%s','now'))",
                        rusqlite::params![m.version, m.name, sum],
                    )
                    .map_err(migration_err)?;
                    Ok(())
                })();
                match applied {
                    Ok(()) => {
                        conn.execute_batch("COMMIT;").map_err(migration_err)?;
                    }
                    Err(e) => {
                        // Best-effort rollback; surface the original error.
                        let _ = conn.execute_batch("ROLLBACK;");
                        return Err(e);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Run all pending migrations against `conn`, transactionally and in order.
///
/// - Creates `_migrations` if absent.
/// - Discovers `NNN_*.sql` files, orders by the numeric prefix.
/// - Applies each unseen version inside its own transaction, recording the
///   sha256 checksum.
/// - Re-applying an already-recorded version is a no-op.
/// - A checksum change on an already-applied version returns `Error::Migration`.
pub fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
    ensure_migrations_table(conn)?;
    let migs = discover()?;
    apply_all(conn, &migs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const M1: &str = "CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT NOT NULL);";

    fn conn() -> rusqlite::Connection {
        rusqlite::Connection::open_in_memory().unwrap()
    }

    fn apply(c: &rusqlite::Connection, migs: &[Migration]) -> Result<()> {
        ensure_migrations_table(c)?;
        apply_all(c, migs)
    }

    fn mig(version: i64, name: &str, sql: &str) -> Migration {
        Migration {
            version,
            name: name.to_string(),
            sql: sql.to_string(),
        }
    }

    #[test]
    fn applies_a_migration_and_records_it() {
        let c = conn();
        apply(&c, &[mig(1, "001_widgets.sql", M1)]).unwrap();

        let cnt: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='widgets'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1, "widgets table should exist");

        let rows: i64 = c
            .query_row(
                "SELECT count(*) FROM _migrations WHERE version=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "version 1 recorded exactly once");
    }

    #[test]
    fn applying_twice_is_a_no_op() {
        let c = conn();
        let migs = [mig(1, "001_widgets.sql", M1)];
        apply(&c, &migs).unwrap();
        apply(&c, &migs).unwrap();

        let rows: i64 = c
            .query_row("SELECT count(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            rows, 1,
            "re-run is a no-op: still exactly one recorded version"
        );
    }

    #[test]
    fn tampered_checksum_errors() {
        let c = conn();
        apply(&c, &[mig(1, "001_widgets.sql", M1)]).unwrap();

        let tampered = [mig(
            1,
            "001_widgets.sql",
            "CREATE TABLE other (id INTEGER);",
        )];
        let err = apply(&c, &tampered).unwrap_err();
        assert!(
            matches!(err, Error::Migration(_)),
            "checksum mismatch must be Error::Migration, got {err:?}"
        );
    }

    #[test]
    fn checksum_is_stable_and_distinct() {
        assert_eq!(checksum("abc"), checksum("abc"));
        assert_ne!(checksum("abc"), checksum("abd"));
        assert_eq!(
            checksum("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn versions_apply_in_numeric_order() {
        let c = conn();
        let migs = [
            mig(2, "002_b.sql", "CREATE TABLE b (id INTEGER);"),
            mig(1, "001_a.sql", "CREATE TABLE a (id INTEGER);"),
            mig(10, "010_c.sql", "CREATE TABLE c (id INTEGER);"),
        ];
        apply(&c, &migs).unwrap();

        let table = |name: &str| -> i64 {
            c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![name],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(table("a"), 1, "version 1 table applied");
        assert_eq!(table("b"), 1, "version 2 table applied");
        assert_eq!(table("c"), 1, "version 10 table applied");

        let recorded: i64 = c
            .query_row("SELECT count(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recorded, 3, "all three versions recorded");
        let max: i64 = c
            .query_row("SELECT max(version) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(max, 10, "two-digit version recorded");
    }
}
