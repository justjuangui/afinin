// SPDX-License-Identifier: MIT OR Apache-2.0

//! Afinín core: read-only AWS cost and inventory analysis.
//!
//! This crate is independent of Tauri so a CLI or TUI can reuse it.

pub mod aws;
pub mod cost;
pub mod error;
pub mod inventory;
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
pub use inventory::graph::{Application, Facet, Graph, GraphFilter, InventoryFacets, TagFilter};
pub use inventory::{Resource, ResourceType};
pub use model::{Account, ConnectionTest, SyncState, SyncStatus, Workspace};
pub use profiles::{AwsFiles, AwsProfile, ProfileKind};
pub use secrets::{KeychainStore, MemoryStore, SecretStore};
pub use settings::Settings;
pub use sso::{SsoLoginPrompt, SsoLoginStatus};
pub use storage::{ApiUsage, InventoryScan, Storage};

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

/// Outcome of syncing one account: costs and inventory run independently.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct AccountSyncReport {
    pub costs: Option<SyncReport>,
    pub inventory: Option<inventory::sync::InventoryReport>,
    pub errors: Vec<String>,
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
    pub async fn sync_account(&self, account_id: &str) -> Result<AccountSyncReport> {
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

        let mut report = AccountSyncReport::default();
        let config = async {
            let profile = self.profile(&account.profile)?;
            aws::sdk_config(&profile, self.secrets.as_ref()).await
        }
        .await;
        match config {
            Err(e) => report.errors.push(e.to_string()),
            Ok(config) => {
                // Costs and inventory are independent: one failing must not hide the other.
                let source = cost::source::CostExplorer::new(&config);
                match cost::sync::sync_account(
                    &self.storage,
                    &source,
                    account_id,
                    &settings,
                    cost::dates::today_utc(),
                )
                .await
                {
                    Ok(r) => report.costs = Some(r),
                    Err(e) => report.errors.push(format!("costs: {e}")),
                }
                if settings.inventory_enabled {
                    match inventory::sync::sync_inventory(
                        &self.storage,
                        &config,
                        account_id,
                        &settings.inventory_regions,
                    )
                    .await
                    {
                        Ok(r) => report.inventory = Some(r),
                        Err(e) => report.errors.push(format!("inventory: {e}")),
                    }
                }
            }
        }

        let error = (!report.errors.is_empty()).then(|| report.errors.join("; "));
        self.db().set_sync_finished(account_id, error.as_deref())?;
        match error {
            Some(e) => {
                tracing::warn!(account = account_id, "sync failed: {e}");
                Err(Error::Other(e))
            }
            None => Ok(report),
        }
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

    // ---- inventory --------------------------------------------------------

    /// Service cost per account over the last 30 days, for the graph overlay.
    fn service_costs(&self, workspace_id: &str) -> Result<inventory::graph::CostOverlay> {
        let today = cost::dates::today_utc();
        let metric = self.settings()?.default_metric;
        let (by_service, unit) = self.db().service_cost_by_account(
            workspace_id,
            metric,
            &cost::dates::format(cost::dates::add_days(today, -30)),
            &cost::dates::format(today),
        )?;
        Ok(inventory::graph::CostOverlay {
            by_service,
            by_resource: Default::default(),
            unit,
        })
    }

    /// Dependency graph with filters and cost overlay (FR-22..FR-24).
    pub fn inventory_graph(&self, workspace_id: &str, filter: &GraphFilter) -> Result<Graph> {
        let (resources, relations) = self.db().workspace_inventory(workspace_id)?;
        let costs = self.service_costs(workspace_id)?;
        Ok(inventory::graph::build(
            &resources, &relations, filter, &costs,
        ))
    }

    /// Resources connected to `arn` within `depth` hops.
    pub fn resource_neighbors(
        &self,
        workspace_id: &str,
        arn: &str,
        depth: usize,
    ) -> Result<Vec<(Resource, usize)>> {
        let (resources, relations) = self.db().workspace_inventory(workspace_id)?;
        Ok(
            inventory::graph::neighbors(&resources, &relations, arn, depth, false)
                .into_iter()
                .map(|(r, d)| (r.clone(), d))
                .collect(),
        )
    }

    /// Applications grouped by a tag key, with last-30-day cost when that tag
    /// is synced from Cost Explorer (FR-25).
    pub fn applications(&self, workspace_id: &str, tag_key: &str) -> Result<Vec<Application>> {
        let (resources, _) = self.db().workspace_inventory(workspace_id)?;
        let mut apps = inventory::graph::applications(&resources, tag_key);
        if self.settings()?.cost_tag_keys.iter().any(|k| k == tag_key) {
            let today = cost::dates::today_utc();
            let series = self.db().cost_series(&CostQuery {
                workspace_id: workspace_id.to_owned(),
                start: cost::dates::format(cost::dates::add_days(today, -30)),
                end: cost::dates::format(today),
                granularity: Granularity::Daily,
                metric: self.settings()?.default_metric,
                group_by: GroupBy::Tag(tag_key.to_owned()),
                filter: CostFilter::default(),
            })?;
            for app in &mut apps {
                app.cost = Some(
                    series
                        .groups
                        .iter()
                        .find(|g| g.key == app.value)
                        .map_or(0.0, |g| g.total),
                );
            }
            // Tag values with cost but no discovered resources still matter.
            for g in &series.groups {
                if !apps.iter().any(|a| a.value == g.key) {
                    apps.push(Application {
                        value: g.key.clone(),
                        resource_count: 0,
                        resource_types: Default::default(),
                        cost: Some(g.total),
                    });
                }
            }
        }
        apps.sort_by(|a, b| {
            b.cost
                .unwrap_or(0.0)
                .total_cmp(&a.cost.unwrap_or(0.0))
                .then(b.resource_count.cmp(&a.resource_count))
        });
        Ok(apps)
    }

    /// Filter options with counts.
    pub fn inventory_facets(&self, workspace_id: &str) -> Result<InventoryFacets> {
        let (resources, _) = self.db().workspace_inventory(workspace_id)?;
        Ok(inventory::graph::facets(&resources))
    }

    pub fn tag_values(&self, workspace_id: &str, key: &str) -> Result<Vec<Facet>> {
        let (resources, _) = self.db().workspace_inventory(workspace_id)?;
        Ok(inventory::graph::tag_values(&resources, key))
    }

    pub fn inventory_scans(&self, workspace_id: &str) -> Result<Vec<InventoryScan>> {
        self.db().inventory_scans(workspace_id)
    }
}
