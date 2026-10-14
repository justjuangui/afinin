// SPDX-License-Identifier: MIT OR Apache-2.0

//! Backup export/import and history retention.

use std::path::Path;

use duckdb::params;

use super::Storage;
use crate::error::{Error, Result};

fn sql_path(p: &Path) -> Result<String> {
    let s = p
        .to_str()
        .ok_or_else(|| Error::Other("path is not valid UTF-8".into()))?;
    Ok(format!("'{}'", s.replace('\'', "''")))
}

impl Storage {
    /// Writes every table to `dir` (schema + Parquet data).
    pub(crate) fn export_to_dir(&self, dir: &Path) -> Result<()> {
        self.conn.execute_batch(&format!(
            "EXPORT DATABASE {} (FORMAT PARQUET);",
            sql_path(dir)?
        ))?;
        Ok(())
    }

    /// Replaces all local data with an export, then upgrades it to the current schema.
    pub(crate) fn import_from_dir(&self, dir: &Path) -> Result<()> {
        if !dir.join("schema.sql").exists() || !dir.join("load.sql").exists() {
            return Err(Error::Other("the backup has no database export".into()));
        }
        let tables: Vec<String> = self
            .conn
            .prepare(
                "SELECT table_name FROM duckdb_tables() WHERE database_name = current_database()",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<duckdb::Result<_>>()?;
        let sequences: Vec<String> = self
            .conn
            .prepare("SELECT sequence_name FROM duckdb_sequences() WHERE database_name = current_database()")?
            .query_map([], |r| r.get(0))?
            .collect::<duckdb::Result<_>>()?;
        let tx = self.conn.unchecked_transaction()?;
        for t in &tables {
            tx.execute_batch(&format!(
                "DROP TABLE IF EXISTS \"{}\" CASCADE;",
                t.replace('"', "\"\"")
            ))?;
        }
        for s in &sequences {
            tx.execute_batch(&format!(
                "DROP SEQUENCE IF EXISTS \"{}\" CASCADE;",
                s.replace('"', "\"\"")
            ))?;
        }
        tx.commit()?;
        self.conn
            .execute_batch(&format!("IMPORT DATABASE {};", sql_path(dir)?))?;
        self.migrate()
    }

    /// Deletes cost history and request logs older than `cutoff` (`YYYY-MM-DD`) (FR-51).
    pub(crate) fn apply_retention(&self, cutoff: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM cost_facts WHERE period_start < ?",
            params![cutoff],
        )?;
        tx.execute(
            "DELETE FROM cur_daily WHERE usage_date < ?",
            params![cutoff],
        )?;
        tx.execute(
            "DELETE FROM cur_periods WHERE billing_period < substr(?, 1, 7)",
            params![cutoff],
        )?;
        tx.execute("DELETE FROM llm_log WHERE created_at < ?", params![cutoff])?;
        tx.execute("DELETE FROM api_calls WHERE called_at < ?", params![cutoff])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::ProfileKind;

    #[test]
    fn export_import_round_trip_between_encrypted_databases() {
        let dir = tempfile::tempdir().unwrap();
        let src = Storage::open(
            &dir.path().join("a.duckdb"),
            "key-a-key-a-key-a-key-a-key-a-00",
        )
        .unwrap();
        let ws = src.create_workspace("Company A").unwrap();
        src.add_account(&ws.id, "Prod", "prod", ProfileKind::Sso)
            .unwrap();
        let export = dir.path().join("export");
        src.export_to_dir(&export).unwrap();

        let dst = Storage::open(
            &dir.path().join("b.duckdb"),
            "key-b-key-b-key-b-key-b-key-b-00",
        )
        .unwrap();
        dst.create_workspace("Will be replaced").unwrap();
        dst.import_from_dir(&export).unwrap();
        let workspaces = dst.list_workspaces().unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "Company A");
        assert_eq!(dst.list_accounts(None).unwrap()[0].name, "Prod");
        // Still writable, and the schema is current.
        dst.create_workspace("After import").unwrap();
    }
}
