// SPDX-License-Identifier: MIT OR Apache-2.0

use duckdb::{OptionalExt, params};

use super::Storage;
use crate::error::{Error, Result};
use crate::settings::Settings;

const KEY: &str = "app";

impl Storage {
    pub fn settings(&self) -> Result<Settings> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?",
                params![KEY],
                |r| r.get(0),
            )
            .optional()?;
        // Unknown or missing fields fall back to defaults.
        Ok(raw
            .and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        let raw = serde_json::to_string(settings).map_err(|e| Error::Other(e.to_string()))?;
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)",
            params![KEY, raw],
        )?;
        Ok(())
    }
}
