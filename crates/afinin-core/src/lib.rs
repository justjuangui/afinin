// SPDX-License-Identifier: MIT OR Apache-2.0

//! Afinín core: read-only AWS cost and inventory analysis.
//!
//! This crate is independent of Tauri so a CLI or TUI can reuse it.

pub mod aws;
pub mod cost;
pub mod error;
pub mod model;
pub mod profiles;
pub mod secrets;
pub mod settings;
pub mod sso;
pub mod storage;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use cost::sync::SyncReport;
pub use cost::{
    CostComparisonRow, CostFilter, CostMetric, CostOverview, CostQuery, CostSeries, DimensionValue,
    Granularity, GroupBy,
};
pub use error::{Error, Result};
pub use model::{Account, ConnectionTest, SyncState, SyncStatus, Workspace};
pub use profiles::{AwsFiles, AwsProfile, ProfileKind};
pub use secrets::{KeychainStore, MemoryStore, SecretStore};
pub use settings::Settings;
pub use sso::{SsoLoginPrompt, SsoLoginStatus};
pub use storage::{ApiUsage, Storage};

/// Where Afinín keeps its local data.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
}

impl AppPaths {
    /// Platform default, e.g. `~/.local/share/afinin` on Linux.
    pub fn platform_default() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "afinin", "afinin")
            .ok_or_else(|| Error::Other("cannot determine the user data directory".into()))?;
        Ok(Self {
            data_dir: dirs.data_dir().to_path_buf(),
        })
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("afinin.duckdb")
    }
}

/// Application facade used by the desktop shell (and future CLI).
pub struct Afinin {
    storage: Mutex<Storage>,
    secrets: Arc<dyn SecretStore>,
    aws_files: AwsFiles,
    pending_sso: Mutex<HashMap<String, sso::PendingLogin>>,
    syncing: Mutex<HashSet<String>>,
}

/// Removes an account from the in-progress set when its sync ends.
struct SyncGuard<'a>(&'a Mutex<HashSet<String>>, String);

impl Drop for SyncGuard<'_> {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.1);
    }
}

impl Afinin {
    /// Opens the encrypted local database, creating its key on first run.
    pub fn open(
        paths: &AppPaths,
        secrets: Arc<dyn SecretStore>,
        aws_files: AwsFiles,
    ) -> Result<Self> {
        let key = secrets::get_or_create_key(secrets.as_ref(), storage::DB_KEY_SECRET)?;
        let storage = Storage::open(&paths.database(), &key)?;
        storage.reset_stale_syncs()?;
        Ok(Self::with_storage(storage, secrets, aws_files))
    }

    pub fn with_storage(
        storage: Storage,
        secrets: Arc<dyn SecretStore>,
        aws_files: AwsFiles,
    ) -> Self {
        Self {
            storage: Mutex::new(storage),
            secrets,
            aws_files,
            pending_sso: Mutex::default(),
            syncing: Mutex::default(),
        }
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Storage> {
        self.storage.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- profiles ---------------------------------------------------------

    pub fn list_profiles(&self) -> Result<Vec<AwsProfile>> {
        profiles::list_profiles(&self.aws_files)
    }

    fn profile(&self, name: &str) -> Result<AwsProfile> {
        profiles::find_profile(&self.aws_files, name)
    }

    // ---- workspaces -------------------------------------------------------

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        self.db().list_workspaces()
    }

    pub fn create_workspace(&self, name: &str) -> Result<Workspace> {
        self.db().create_workspace(name)
    }

    pub fn rename_workspace(&self, id: &str, name: &str) -> Result<()> {
        self.db().rename_workspace(id, name)
    }

    pub fn delete_workspace(&self, id: &str) -> Result<()> {
        self.db().delete_workspace(id)
    }

    // ---- accounts ---------------------------------------------------------

    pub fn list_accounts(&self, workspace_id: Option<&str>) -> Result<Vec<Account>> {
        self.db().list_accounts(workspace_id)
    }

    /// Adds an account backed by an existing AWS profile.
    pub fn add_account(
        &self,
        workspace_id: &str,
        profile: &str,
        name: Option<&str>,
    ) -> Result<Account> {
        let profile = self.profile(profile)?;
        let name = name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&profile.name);
        self.db()
            .add_account(workspace_id, name, &profile.name, profile.kind)
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        self.db().remove_account(id)
    }

    /// Runs the connection test for an account and stores the result.
    pub async fn test_connection(&self, account_id: &str) -> Result<Account> {
        let account = self.db().get_account(account_id)?;
        let profile = self.profile(&account.profile)?;
        let test = aws::test_connection(&profile, self.secrets.as_ref()).await;
        let db = self.db();
        db.record_connection_test(account_id, &test)?;
        db.get_account(account_id)
    }

    // ---- SSO --------------------------------------------------------------

    fn sso_settings(&self, profile: &str) -> Result<profiles::SsoSettings> {
        self.profile(profile)?
            .sso
            .ok_or_else(|| Error::InvalidProfile(format!("profile `{profile}` does not use SSO")))
    }

    /// Starts the browser-based SSO login for a profile.
    pub async fn start_sso_login(&self, profile: &str) -> Result<SsoLoginPrompt> {
        let settings = self.sso_settings(profile)?;
        let (pending, prompt) = sso::start(&settings).await?;
        self.pending_sso
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(profile.to_owned(), pending);
        Ok(prompt)
    }

    /// Polls a login started with [`Afinin::start_sso_login`].
    pub async fn poll_sso_login(&self, profile: &str) -> Result<SsoLoginStatus> {
        let pending = self
            .pending_sso
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(profile)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("no SSO login in progress for `{profile}`")))?;
        let status = sso::poll(&pending, self.secrets.as_ref()).await?;
        if status != SsoLoginStatus::Pending {
            self.pending_sso
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(profile);
        }
        Ok(status)
    }

    /// Forgets the SSO token stored for a profile's session.
    pub fn sso_logout(&self, profile: &str) -> Result<()> {
        let settings = self.sso_settings(profile)?;
        self.secrets.delete(&aws::sso_secret_key(&settings))
    }

    // ---- settings ---------------------------------------------------------

    pub fn settings(&self) -> Result<Settings> {
        self.db().settings()
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.db().save_settings(settings)
    }

    // ---- sync -------------------------------------------------------------

    /// Fetches Cost Explorer data for one account (FR-14, FR-50).
    pub async fn sync_account(&self, account_id: &str) -> Result<SyncReport> {
        let _guard = {
            let mut syncing = self.syncing.lock().unwrap_or_else(|e| e.into_inner());
            if !syncing.insert(account_id.to_owned()) {
                return Err(Error::Other(
                    "a sync for this account is already running".into(),
                ));
            }
            SyncGuard(&self.syncing, account_id.to_owned())
        };
        let (account, settings) = {
            let db = self.db();
            (db.get_account(account_id)?, db.settings()?)
        };
        self.db().set_sync_running(account_id)?;

        let result = async {
            let profile = self.profile(&account.profile)?;
            let config = aws::sdk_config(&profile, self.secrets.as_ref()).await?;
            let source = cost::source::CostExplorer::new(&config);
            cost::sync::sync_account(
                &self.storage,
                &source,
                account_id,
                &settings,
                cost::dates::today_utc(),
            )
            .await
        }
        .await;

        let error = result.as_ref().err().map(ToString::to_string);
        self.db().set_sync_finished(account_id, error.as_deref())?;
        if let Err(e) = &result {
            tracing::warn!(account = account_id, "sync failed: {e}");
        }
        result
    }

    /// Accounts whose last sync attempt is older than the configured interval.
    pub fn accounts_due_for_sync(&self) -> Result<Vec<String>> {
        let db = self.db();
        let hours = db.settings()?.sync_interval_hours;
        if hours == 0 {
            return Ok(Vec::new());
        }
        let cutoff = time::OffsetDateTime::now_utc() - time::Duration::hours(hours.into());
        let cutoff = cutoff
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok(db
            .list_accounts(None)?
            .into_iter()
            .filter(|a| match &a.sync {
                None => true,
                Some(s) => s.state != SyncState::Running && s.started_at < cutoff,
            })
            .map(|a| a.id)
            .collect())
    }

    // ---- costs ------------------------------------------------------------

    pub fn cost_overview(
        &self,
        workspace_id: &str,
        metric: CostMetric,
        linked_accounts: &[String],
    ) -> Result<CostOverview> {
        self.db().cost_overview(
            workspace_id,
            metric,
            linked_accounts,
            cost::dates::today_utc(),
        )
    }

    pub fn cost_series(&self, query: &CostQuery) -> Result<CostSeries> {
        self.db().cost_series(query)
    }

    pub fn compare_costs(
        &self,
        query: &CostQuery,
        prev_start: &str,
        prev_end: &str,
    ) -> Result<Vec<CostComparisonRow>> {
        self.db().compare_costs(query, prev_start, prev_end)
    }

    pub fn dimension_values(
        &self,
        workspace_id: &str,
        group_by: &GroupBy,
    ) -> Result<Vec<DimensionValue>> {
        self.db().dimension_values(workspace_id, group_by)
    }

    /// CSV of a breakdown (FR-16).
    pub fn export_series_csv(&self, query: &CostQuery) -> Result<String> {
        Ok(cost::csv::series_to_csv(&self.cost_series(query)?))
    }

    pub fn export_comparison_csv(
        &self,
        query: &CostQuery,
        prev_start: &str,
        prev_end: &str,
    ) -> Result<String> {
        Ok(cost::csv::comparison_to_csv(
            &self.compare_costs(query, prev_start, prev_end)?,
        ))
    }

    /// Billable AWS calls made by Afinín this calendar month (NFR-30).
    pub fn api_usage_this_month(&self) -> Result<Vec<ApiUsage>> {
        let month_start = cost::dates::first_of_month(cost::dates::today_utc());
        self.db().api_usage(&cost::dates::format(month_start))
    }
}
