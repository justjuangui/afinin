// SPDX-License-Identifier: MIT OR Apache-2.0

//! Local storage on DuckDB, encrypted at rest (NFR-2, D-1). The key is generated
//! on first run and kept only in the OS keychain.

use std::path::Path;

use duckdb::{Connection, OptionalExt, params};

use crate::error::{Error, Result};
use crate::model::{Account, ConnectionTest, Workspace, now_rfc3339};
use crate::profiles::ProfileKind;

/// Keychain entry holding the database encryption key.
pub const DB_KEY_SECRET: &str = "db-encryption-key";

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE workspaces (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL UNIQUE,
        created_at  TEXT NOT NULL
    );
    CREATE TABLE accounts (
        id              TEXT PRIMARY KEY,
        -- No FOREIGN KEY: DuckDB cannot delete a parent and its children in one
        -- transaction. Integrity is enforced in `add_account`/`delete_workspace`.
        workspace_id    TEXT NOT NULL,
        name            TEXT NOT NULL,
        profile         TEXT NOT NULL,
        kind            TEXT NOT NULL,
        aws_account_id  TEXT,
        test_ok         BOOLEAN,
        tested_at       TEXT,
        test_arn        TEXT,
        test_error      TEXT,
        created_at      TEXT NOT NULL,
        UNIQUE (workspace_id, profile)
    );
    "#,
];

pub struct Storage {
    conn: Connection,
}

fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

impl Storage {
    /// Opens (or creates) the encrypted database at `path`.
    pub fn open(path: &Path, key: &str) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open_in_memory()?;
        let path = path
            .to_str()
            .ok_or_else(|| Error::Other("database path is not valid UTF-8".into()))?;
        conn.execute_batch(&format!(
            "ATTACH {} AS afinin (ENCRYPTION_KEY {}); USE afinin;",
            sql_string(path),
            sql_string(key)
        ))?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    /// Unencrypted in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let storage = Self {
            conn: Connection::open_in_memory()?,
        };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )?;
        let current: i64 = self.conn.query_row(
            "SELECT coalesce(max(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_version VALUES (?)",
                params![i as i64 + 1],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    // ---- workspaces -------------------------------------------------------

    pub fn create_workspace(&self, name: &str) -> Result<Workspace> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other("workspace name cannot be empty".into()));
        }
        let ws = Workspace {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_owned(),
            created_at: now_rfc3339(),
        };
        self.conn.execute(
            "INSERT INTO workspaces (id, name, created_at) VALUES (?, ?, ?)",
            params![ws.id, ws.name, ws.created_at],
        )?;
        Ok(ws)
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, created_at FROM workspaces ORDER BY created_at, name")?;
        let rows = stmt.query_map([], |r| {
            Ok(Workspace {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    pub fn rename_workspace(&self, id: &str, name: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE workspaces SET name = ? WHERE id = ?",
            params![name.trim(), id],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("workspace {id}")));
        }
        Ok(())
    }

    /// Deletes a workspace and all its accounts.
    pub fn delete_workspace(&self, id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM accounts WHERE workspace_id = ?", params![id])?;
        let n = tx.execute("DELETE FROM workspaces WHERE id = ?", params![id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("workspace {id}")));
        }
        tx.commit()?;
        Ok(())
    }

    // ---- accounts ---------------------------------------------------------

    pub fn add_account(
        &self,
        workspace_id: &str,
        name: &str,
        profile: &str,
        kind: ProfileKind,
    ) -> Result<Account> {
        let exists: bool = self.conn.query_row(
            "SELECT count(*) > 0 FROM workspaces WHERE id = ?",
            params![workspace_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Error::NotFound(format!("workspace {workspace_id}")));
        }
        let account = Account {
            id: uuid::Uuid::new_v4().to_string(),
            workspace_id: workspace_id.to_owned(),
            name: name.trim().to_owned(),
            profile: profile.to_owned(),
            kind,
            aws_account_id: None,
            last_test: None,
            created_at: now_rfc3339(),
        };
        self.conn.execute(
            "INSERT INTO accounts (id, workspace_id, name, profile, kind, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                account.id,
                account.workspace_id,
                account.name,
                account.profile,
                kind_to_str(kind),
                account.created_at
            ],
        )?;
        Ok(account)
    }

    pub fn list_accounts(&self, workspace_id: Option<&str>) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workspace_id, name, profile, kind, aws_account_id,
                    test_ok, tested_at, test_arn, test_error, created_at
             FROM accounts
             WHERE ?1 IS NULL OR workspace_id = ?1
             ORDER BY name",
        )?;
        let rows = stmt.query_map(params![workspace_id], row_to_account)?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }

    pub fn get_account(&self, id: &str) -> Result<Account> {
        self.conn
            .query_row(
                "SELECT id, workspace_id, name, profile, kind, aws_account_id,
                        test_ok, tested_at, test_arn, test_error, created_at
                 FROM accounts WHERE id = ?",
                params![id],
                row_to_account,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("account {id}")))
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM accounts WHERE id = ?", params![id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("account {id}")));
        }
        Ok(())
    }

    pub fn record_connection_test(&self, id: &str, test: &ConnectionTest) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET
                test_ok = ?, tested_at = ?, test_arn = ?, test_error = ?,
                aws_account_id = coalesce(?, aws_account_id)
             WHERE id = ?",
            params![
                test.ok,
                test.tested_at,
                test.arn,
                test.error,
                test.aws_account_id,
                id
            ],
        )?;
        Ok(())
    }
}

fn kind_to_str(kind: ProfileKind) -> &'static str {
    match kind {
        ProfileKind::Sso => "sso",
        ProfileKind::AssumeRole => "assume_role",
        ProfileKind::CredentialProcess => "credential_process",
        ProfileKind::WebIdentity => "web_identity",
        ProfileKind::StaticKeys => "static_keys",
        ProfileKind::Unknown => "unknown",
    }
}

fn kind_from_str(s: &str) -> ProfileKind {
    match s {
        "sso" => ProfileKind::Sso,
        "assume_role" => ProfileKind::AssumeRole,
        "credential_process" => ProfileKind::CredentialProcess,
        "web_identity" => ProfileKind::WebIdentity,
        "static_keys" => ProfileKind::StaticKeys,
        _ => ProfileKind::Unknown,
    }
}

fn row_to_account(r: &duckdb::Row<'_>) -> duckdb::Result<Account> {
    let kind: String = r.get(4)?;
    let test_ok: Option<bool> = r.get(6)?;
    let tested_at: Option<String> = r.get(7)?;
    let aws_account_id: Option<String> = r.get(5)?;
    let last_test = match (test_ok, tested_at) {
        (Some(ok), Some(tested_at)) => Some(ConnectionTest {
            ok,
            tested_at,
            aws_account_id: aws_account_id.clone(),
            arn: r.get(8)?,
            error: r.get(9)?,
        }),
        _ => None,
    };
    Ok(Account {
        id: r.get(0)?,
        workspace_id: r.get(1)?,
        name: r.get(2)?,
        profile: r.get(3)?,
        kind: kind_from_str(&kind),
        aws_account_id,
        last_test,
        created_at: r.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_and_account_crud() {
        let db = Storage::open_in_memory().unwrap();
        let ws = db.create_workspace("Company A").unwrap();
        assert_eq!(db.list_workspaces().unwrap(), vec![ws.clone()]);

        let acc = db
            .add_account(&ws.id, "Prod", "prod", ProfileKind::Sso)
            .unwrap();
        assert_eq!(db.list_accounts(Some(&ws.id)).unwrap().len(), 1);
        assert_eq!(db.list_accounts(None).unwrap().len(), 1);
        assert!(
            db.add_account(&ws.id, "Dup", "prod", ProfileKind::Sso)
                .is_err()
        );
        assert!(matches!(
            db.add_account("missing", "X", "x", ProfileKind::Sso),
            Err(Error::NotFound(_))
        ));

        let test = ConnectionTest {
            ok: true,
            tested_at: now_rfc3339(),
            aws_account_id: Some("222222222222".into()),
            arn: Some("arn:aws:sts::222222222222:assumed-role/ViewOnly/me".into()),
            error: None,
        };
        db.record_connection_test(&acc.id, &test).unwrap();
        let acc = db.get_account(&acc.id).unwrap();
        assert_eq!(acc.aws_account_id.as_deref(), Some("222222222222"));
        assert_eq!(acc.last_test, Some(test));

        db.delete_workspace(&ws.id).unwrap();
        assert!(db.list_accounts(None).unwrap().is_empty());
        assert!(matches!(db.get_account(&acc.id), Err(Error::NotFound(_))));
    }

    #[test]
    fn encrypted_file_requires_key_and_hides_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("afinin.duckdb");
        let key = "0123456789abcdef0123456789abcdef";
        {
            let db = Storage::open(&path, key).unwrap();
            db.create_workspace("VerySecretWorkspaceName").unwrap();
            db.conn.execute_batch("CHECKPOINT;").unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        let needle = b"VerySecretWorkspaceName";
        assert!(!bytes.windows(needle.len()).any(|w| w == needle));

        assert!(Storage::open(&path, "wrong-key-wrong-key-wrong-key-00").is_err());

        let db = Storage::open(&path, key).unwrap();
        assert_eq!(
            db.list_workspaces().unwrap()[0].name,
            "VerySecretWorkspaceName"
        );
    }
}
