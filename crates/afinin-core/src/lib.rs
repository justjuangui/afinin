// SPDX-License-Identifier: MIT OR Apache-2.0

//! Afinín core: read-only AWS cost and inventory analysis.
//!
//! This crate is independent of Tauri so a CLI or TUI can reuse it.

pub mod aws;
pub mod backup;
pub mod cost;
pub mod cur;
pub mod error;
pub mod findings;
pub mod inventory;
pub mod llm;
pub mod model;
pub mod permissions;
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
pub use cur::{CurConfig, CurValidation};
pub use error::{Error, Result};
pub use findings::{Finding, FindingStatus};
pub use inventory::graph::{Application, Facet, Graph, GraphFilter, InventoryFacets, TagFilter};
pub use inventory::{Resource, ResourceType};
pub use llm::agent::{Citation, Turn};
pub use llm::context::{Entry as ContextEntry, ScreenContext, StoredContext, WorkspaceContext};
pub use llm::{LlmSettings, ProviderKind};
pub use model::{Account, ConnectionTest, SyncState, SyncStatus, Workspace};
pub use profiles::{AwsFiles, AwsProfile, ProfileKind};
pub use secrets::{KeychainStore, MemoryStore, SecretStore};
pub use settings::Settings;
pub use sso::{SsoLoginPrompt, SsoLoginStatus};
pub use storage::{
    ApiUsage, ContextRevision, ConversationSummary, CurStatus, InventoryScan, LlmLogEntry, Rating,
    Storage, StoredTurn,
};

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
    /// Scratch space for CUR downloads (deleted right after loading).
    tmp_dir: PathBuf,
    /// Assistant conversations in use. Answers are saved; a conversation not
    /// in memory (e.g. after a restart) is rebuilt from them.
    conversations: Mutex<HashMap<String, llm::agent::Conversation>>,
}

/// Keychain entry for the LLM provider's API key (FR-2).
const LLM_KEY_SECRET: &str = "llm-api-key";

/// Outcome of syncing one account: costs and inventory run independently.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct AccountSyncReport {
    pub costs: Option<SyncReport>,
    pub inventory: Option<inventory::sync::InventoryReport>,
    pub cur: Option<cur::CurReport>,
    pub errors: Vec<String>,
    /// Non-fatal problems (prices, commitments) that don't fail the sync.
    pub warnings: Vec<String>,
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
        let mut afinin = Self::with_storage(storage, secrets, aws_files);
        afinin.tmp_dir = paths.data_dir.join("tmp");
        Ok(afinin)
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
            tmp_dir: std::env::temp_dir().join("afinin"),
            conversations: Mutex::default(),
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
                let cur_config = self.db().cur_config(account_id)?;
                if let Some(cfg) = cur_config {
                    let store = cur::source::S3Store::new(&config, &cfg);
                    match cur::ingest::ingest(&self.storage, &store, &cfg, &self.tmp_dir).await {
                        Ok(r) => report.cur = Some(r),
                        Err(e) => report.errors.push(format!("CUR: {e}")),
                    }
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

        if report.inventory.is_some() || report.costs.is_some() {
            let config = async {
                let profile = self.profile(&account.profile)?;
                aws::sdk_config(&profile, self.secrets.as_ref()).await
            }
            .await;
            if let Ok(config) = config {
                report.warnings.extend(
                    self.refresh_finding_inputs(&config, account_id, &settings)
                        .await,
                );
            }
        }

        if settings.retention_months > 0 {
            let cutoff = cost::dates::format(cost::dates::add_months(
                cost::dates::today_utc(),
                -(settings.retention_months as i32),
            ));
            if let Err(e) = self.db().apply_retention(&cutoff) {
                report.warnings.push(format!("retention: {e}"));
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
            // With CUR, the overlay shows each resource's own cost.
            by_resource: self.db().cur_resource_costs(
                workspace_id,
                metric,
                &cost::dates::format(cost::dates::add_days(today, -30)),
                &cost::dates::format(today),
            )?,
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

    // ---- CUR (FR-15) ------------------------------------------------------

    /// Checks an export with read-only calls, using the account's credentials.
    pub async fn validate_cur(&self, cfg: &cur::CurConfig) -> Result<cur::CurValidation> {
        let account = self.db().get_account(&cfg.account_id)?;
        let profile = self.profile(&account.profile)?;
        let config = aws::sdk_config(&profile, self.secrets.as_ref()).await?;
        Ok(cur::source::validate(&config, cfg).await)
    }

    pub fn save_cur_config(&self, cfg: &cur::CurConfig) -> Result<()> {
        self.db().save_cur_config(cfg)
    }

    pub fn cur_config(&self, account_id: &str) -> Result<Option<cur::CurConfig>> {
        self.db().cur_config(account_id)
    }

    pub fn delete_cur_config(&self, account_id: &str) -> Result<()> {
        self.db().delete_cur_config(account_id)
    }

    pub fn cur_status(&self, workspace_id: &str) -> Result<CurStatus> {
        self.db().cur_status(workspace_id)
    }

    // ---- findings (FR-30..FR-34) -----------------------------------------

    /// Refreshes public prices, commitment coverage and (opt-in) AWS anomalies.
    async fn refresh_finding_inputs(
        &self,
        config: &aws_config::SdkConfig,
        account_id: &str,
        settings: &Settings,
    ) -> Vec<String> {
        use findings::pricing::PriceItem;
        let mut warnings = Vec::new();

        // Public prices for what this account actually has (Price List API is free).
        let needed: std::collections::BTreeMap<String, std::collections::BTreeSet<PriceItem>> = {
            let db = self.db();
            let account = db.get_account(account_id).ok();
            let ws = account.map(|a| a.workspace_id).unwrap_or_default();
            let (resources, _) = db.workspace_inventory(&ws).unwrap_or_default();
            let mut needed: std::collections::BTreeMap<
                String,
                std::collections::BTreeSet<PriceItem>,
            > = Default::default();
            for r in &resources {
                let items = needed.entry(r.region.clone()).or_default();
                let attr = |k: &str| {
                    r.attrs
                        .get(k)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned()
                };
                match r.resource_type {
                    ResourceType::EbsVolume => {
                        let t = attr("volume_type");
                        items.insert(PriceItem::EbsStorage(t.clone()));
                        if matches!(t.as_str(), "gp3" | "io1" | "io2") {
                            items.insert(PriceItem::EbsIops(t.clone()));
                        }
                        if t == "gp3" {
                            items.insert(PriceItem::EbsThroughput);
                        }
                    }
                    ResourceType::EbsSnapshot => {
                        items.insert(PriceItem::EbsSnapshot);
                    }
                    ResourceType::ElasticIp => {
                        items.insert(PriceItem::PublicIpv4Idle);
                    }
                    ResourceType::NatGateway => {
                        items.insert(PriceItem::NatGatewayHour);
                    }
                    ResourceType::LoadBalancer => {
                        items.insert(PriceItem::LoadBalancerHour(attr("type")));
                    }
                    _ => {}
                }
            }
            needed.retain(|region, items| region != "global" && !items.is_empty());
            needed
        };
        let cutoff = (time::OffsetDateTime::now_utc()
            - time::Duration::days(storage::PRICE_TTL_DAYS))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
        for (region, items) in needed {
            let fresh = self
                .db()
                .fresh_price_keys(&region, &cutoff)
                .unwrap_or_default();
            let stale: Vec<PriceItem> = items
                .into_iter()
                .filter(|i| !fresh.contains(&i.key()))
                .collect();
            if stale.is_empty() {
                continue;
            }
            match findings::pricing::fetch(config, &region, &stale).await {
                Ok(prices) => {
                    let rows: Vec<(String, f64)> =
                        prices.iter().map(|(i, p)| (i.key(), *p)).collect();
                    if let Err(e) = self.db().save_prices(&region, &rows) {
                        warnings.push(format!("prices: {e}"));
                    }
                }
                Err(e) => {
                    warnings.push(format!(
                        "public prices ({region}): {e}; using built-in list prices"
                    ));
                    break;
                }
            }
        }

        let today = cost::dates::today_utc();
        let (start, end) = (
            cost::dates::format(cost::dates::add_days(today, -30)),
            cost::dates::format(today),
        );
        if settings.commitments_enabled {
            let week_ago = (time::OffsetDateTime::now_utc() - time::Duration::days(7))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            let due = self
                .db()
                .commitments_fetched_at(account_id)
                .ok()
                .flatten()
                .is_none_or(|t| t < week_ago);
            if due {
                let (summaries, calls, errs) =
                    findings::commitments::fetch(config, &start, &end).await;
                let db = self.db();
                let _ = db.record_api_call(
                    account_id,
                    "ce",
                    "Get*Coverage/Utilization",
                    calls as f64 * cost::source::CE_REQUEST_COST_USD,
                );
                if let Err(e) = db.save_commitments(account_id, &summaries) {
                    warnings.push(format!("commitments: {e}"));
                }
                warnings.extend(errs);
            }
        }
        if settings.import_aws_anomalies {
            match findings::commitments::aws_anomalies(config, &start).await {
                Ok((anomalies, calls)) => {
                    let db = self.db();
                    let _ = db.record_api_call(
                        account_id,
                        "ce",
                        "GetAnomalies",
                        calls as f64 * cost::source::CE_REQUEST_COST_USD,
                    );
                    if let Err(e) = db.save_aws_anomalies(account_id, &anomalies) {
                        warnings.push(format!("AWS anomalies: {e}"));
                    }
                }
                Err(e) => warnings.push(format!("AWS anomalies: {e}")),
            }
        }
        warnings
    }

    /// All findings for a workspace, newest state applied (FR-33). Dismissed
    /// and snoozed findings are included only when `include_hidden`.
    pub fn findings(&self, workspace_id: &str, include_hidden: bool) -> Result<Vec<Finding>> {
        let today = cost::dates::today_utc();
        let today_s = cost::dates::format(today);
        let settings = self.settings()?;
        let db = self.db();
        let (resources, relations) = db.workspace_inventory(workspace_id)?;
        let edges = inventory::graph::resolve(&resources, &relations);
        let prices = db.price_book()?;
        let window = (
            cost::dates::format(cost::dates::add_days(today, -30)),
            today_s.clone(),
        );
        let actual =
            db.cur_resource_costs(workspace_id, CostMetric::NetUnblended, &window.0, &window.1)?;
        let nat = db.cur_nat_usage(workspace_id, &window.0, &window.1)?;
        let empty = std::collections::HashMap::new();

        let mut all = findings::rules::evaluate(&findings::rules::RuleInput {
            resources: &resources,
            edges: &edges,
            prices: &prices,
            actual_costs: &actual,
            nat_gb: nat.as_ref().map(|n| &n.0),
            nat_processing_cost: nat.as_ref().map_or(&empty, |n| &n.1),
            snapshot_age_days: settings.snapshot_age_days,
            today,
        });

        let daily = db.daily_service_costs(
            workspace_id,
            settings.default_metric,
            &cost::dates::format(cost::dates::add_days(
                today,
                -(findings::anomalies::BASELINE_DAYS + findings::anomalies::RECENT_DAYS),
            )),
            &today_s,
        )?;
        all.extend(findings::anomalies::detect(
            &daily,
            settings.anomaly_sensitivity,
            today,
        ));

        for (_, a) in db.aws_anomalies(workspace_id)? {
            let mut f =
                findings::Finding::new("aws-anomaly", findings::FindingKind::Anomaly, &a.id)
                    .evidence("start", &a.start)
                    .evidence("end", &a.end)
                    .evidence("root_causes", &a.root_causes)
                    .evidence("source", "AWS Cost Anomaly Detection");
            f.impact = Some(a.impact);
            f.severity = findings::Severity::from_amount(a.impact);
            f.title = format!(
                "AWS detected a cost anomaly{}",
                a.dimension
                    .as_deref()
                    .map(|d| format!(" in {d}"))
                    .unwrap_or_default()
            );
            f.summary = format!(
                "Estimated impact {:.2}{}.",
                a.impact,
                a.root_causes
                    .first()
                    .map(|c| format!("; top root cause: {c}"))
                    .unwrap_or_default()
            );
            f.how_to_fix = "Open the anomaly in the AWS Cost Anomaly Detection console for details and give AWS feedback if it's expected.".into();
            all.push(f);
        }

        let ids: Vec<&str> = all.iter().map(|f| f.id.as_str()).collect();
        let seen = db.mark_findings_seen(&ids, &today_s)?;
        let states = db.finding_states()?;
        for f in &mut all {
            f.first_seen = seen.get(&f.id).cloned();
            if let Some(s) = states.get(&f.id) {
                f.status = s.clone();
            }
            // An expired snooze reopens the finding.
            if let findings::FindingStatus::Snoozed { until } = &f.status
                && *until <= today_s
            {
                f.status = findings::FindingStatus::Open;
            }
        }
        if !include_hidden {
            all.retain(|f| f.status == findings::FindingStatus::Open);
        }
        all.sort_by(|a, b| {
            b.severity.cmp(&a.severity).then_with(|| {
                b.monthly_savings
                    .or(b.impact)
                    .unwrap_or(0.0)
                    .total_cmp(&a.monthly_savings.or(a.impact).unwrap_or(0.0))
            })
        });
        Ok(all)
    }

    pub fn set_finding_status(&self, finding_id: &str, status: &FindingStatus) -> Result<()> {
        self.db().set_finding_status(finding_id, status)
    }

    /// Patterns and trends (FR-32).
    pub fn insights(&self, workspace_id: &str) -> Result<Insights> {
        let today = cost::dates::today_utc();
        let d = |n| cost::dates::format(cost::dates::add_days(today, n));
        let metric = self.settings()?.default_metric;
        let query = |group_by: GroupBy, start: String, end: String| CostQuery {
            workspace_id: workspace_id.to_owned(),
            start,
            end,
            granularity: Granularity::Daily,
            metric,
            group_by,
            filter: CostFilter::default(),
        };
        let db = self.db();
        let mut top_movers =
            db.compare_costs(&query(GroupBy::Service, d(-30), d(0)), &d(-60), &d(-30))?;
        // Changes that would display as 0.00 are noise.
        top_movers.retain(|r| r.delta.abs() >= 0.005);
        top_movers.truncate(10);
        let usage = db.cost_series(&query(GroupBy::UsageType, d(-30), d(0)))?;
        let mut data_transfer: Vec<insights::Labeled> = usage
            .groups
            .iter()
            .filter(|g| insights::is_data_transfer(&g.key) && g.total >= 0.005)
            .map(|g| insights::Labeled {
                key: g.key.clone(),
                label: insights::describe_usage_type(&g.key),
                total: g.total,
            })
            .collect();
        data_transfer.truncate(15);
        let labels: std::collections::HashMap<String, String> = db
            .list_accounts(Some(workspace_id))?
            .into_iter()
            .map(|a| (a.id, a.name))
            .collect();
        let commitments = db
            .commitments(workspace_id)?
            .into_iter()
            .map(|(acct, s)| (labels.get(&acct).cloned().unwrap_or(acct), s))
            .collect();
        Ok(Insights {
            top_movers,
            data_transfer,
            data_transfer_total: usage
                .groups
                .iter()
                .filter(|g| insights::is_data_transfer(&g.key))
                .map(|g| g.total)
                .sum(),
            commitments,
            unit: usage.unit,
        })
    }

    // ---- backup (FR-53) --------------------------------------------------

    /// Writes all local data (never credentials) to a password-encrypted file.
    pub fn export_backup(&self, path: &std::path::Path, password: &str) -> Result<()> {
        if password.chars().count() < backup::MIN_PASSWORD_LEN {
            return Err(Error::Other(format!(
                "use a password of at least {} characters",
                backup::MIN_PASSWORD_LEN
            )));
        }
        std::fs::create_dir_all(&self.tmp_dir)?;
        let tmp = backup::temp_dir(&self.tmp_dir)?;
        let export = tmp.0.join("db");
        self.db().export_to_dir(&export)?;
        let archive = backup::tar_dir(&export)?;
        drop(tmp);
        std::fs::write(path, backup::encrypt(&archive, password)?)?;
        Ok(())
    }

    /// Replaces all local data with a backup. Accounts reference AWS profile
    /// names, which must exist on this machine to sync.
    pub fn import_backup(&self, path: &std::path::Path, password: &str) -> Result<()> {
        let archive = backup::decrypt(&std::fs::read(path)?, password)?;
        std::fs::create_dir_all(&self.tmp_dir)?;
        let tmp = backup::temp_dir(&self.tmp_dir)?;
        backup::untar(&archive, &tmp.0)?;
        self.db().import_from_dir(&tmp.0)?;
        self.conversations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        Ok(())
    }

    // ---- assistant (FR-40..FR-46) -----------------------------------------

    pub fn set_llm_api_key(&self, key: Option<&str>) -> Result<()> {
        match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => self.secrets.set(LLM_KEY_SECRET, k),
            None => self.secrets.delete(LLM_KEY_SECRET),
        }
    }

    pub fn has_llm_api_key(&self) -> Result<bool> {
        Ok(self.secrets.get(LLM_KEY_SECRET)?.is_some())
    }

    async fn llm_provider(&self, cfg: &LlmSettings) -> Result<Box<dyn llm::Provider>> {
        let key = self.secrets.get(LLM_KEY_SECRET)?;
        Ok(match cfg.provider {
            ProviderKind::None => {
                return Err(Error::Other(
                    "no assistant is configured; choose a provider in Settings".into(),
                ));
            }
            ProviderKind::OpenAiCompatible => Box::new(llm::providers::OpenAiCompatible::new(
                &cfg.base_url,
                cfg.model(),
                key,
            )?),
            ProviderKind::Anthropic => Box::new(llm::providers::Anthropic::new(
                cfg.model(),
                key.unwrap_or_default(),
            )?),
            ProviderKind::Bedrock => {
                let account_id = cfg.bedrock_account_id.as_deref().ok_or_else(|| {
                    Error::Other("choose which connected account calls Bedrock".into())
                })?;
                let account = self.db().get_account(account_id)?;
                let config =
                    aws::sdk_config(&self.profile(&account.profile)?, self.secrets.as_ref())
                        .await?;
                Box::new(llm::providers::Bedrock::new(
                    &config,
                    &cfg.bedrock_region,
                    cfg.model(),
                ))
            }
        })
    }

    /// Strings to mask besides pattern-matched identifiers: tag values and
    /// resource names discovered in the workspace.
    fn redaction_names(&self, workspace_id: &str) -> Result<Vec<String>> {
        let (resources, _) = self.db().workspace_inventory(workspace_id)?;
        let mut names: std::collections::BTreeSet<String> = Default::default();
        for r in &resources {
            names.extend(r.tags.values().cloned());
            if let Some(n) = &r.name {
                names.insert(n.clone());
            }
        }
        for a in self.db().linked_account_values(workspace_id)? {
            // Account names: "Name (123456789012)" → "Name".
            if let Some((name, _)) = a.label.rsplit_once(" (") {
                names.insert(name.to_owned());
            }
        }
        Ok(names.into_iter().collect())
    }

    pub fn workspace_context(&self, workspace_id: &str) -> Result<StoredContext> {
        self.db().workspace_context(workspace_id)
    }

    /// Saves a new revision of the workspace context (unchanged text is not a
    /// new revision).
    pub fn save_workspace_context(
        &self,
        workspace_id: &str,
        context: WorkspaceContext,
    ) -> Result<StoredContext> {
        self.db().save_workspace_context(workspace_id, context)
    }

    pub fn workspace_context_history(&self, workspace_id: &str) -> Result<Vec<ContextRevision>> {
        self.db().workspace_context_history(workspace_id, 50)
    }

    /// The workspace context rendered for the prompt, with account names.
    fn rendered_context(&self, workspace_id: &str) -> Result<(String, i64)> {
        let stored = self.db().workspace_context(workspace_id)?;
        if stored.context.is_empty() {
            return Ok((String::new(), stored.revision));
        }
        let names: HashMap<String, String> = self
            .db()
            .linked_account_values(workspace_id)?
            .into_iter()
            .filter_map(|a| {
                let (name, _) = a.label.rsplit_once(" (")?;
                Some((a.key, name.to_owned()))
            })
            .collect();
        Ok((
            stored.context.render(|id| names.get(id).cloned()),
            stored.revision,
        ))
    }

    /// Asks the assistant (FR-41). Pass the previous turn's `conversation_id`
    /// to continue a conversation. `on_progress` reports each model call and
    /// tool call while the turn runs.
    pub async fn ask(
        &self,
        workspace_id: &str,
        conversation_id: Option<&str>,
        question: &str,
        screen: Option<&llm::context::ScreenContext>,
        on_progress: &(dyn Fn(llm::agent::Progress) + Send + Sync),
    ) -> Result<Turn> {
        self.answer(
            workspace_id,
            conversation_id,
            question,
            screen,
            on_progress,
            true,
        )
        .await
    }

    /// Re-asks a reference answer's question in a fresh conversation, with the
    /// screen it was asked from, and compares the result. The new answer is
    /// not saved to History (its requests are in the request log).
    pub async fn recheck_reference(
        &self,
        workspace_id: &str,
        turn_id: i64,
        on_progress: &(dyn Fn(llm::agent::Progress) + Send + Sync),
    ) -> Result<llm::recheck::Recheck> {
        let saved = self.db().assistant_turn(turn_id)?;
        let after = self
            .answer(
                workspace_id,
                None,
                &saved.question,
                saved.screen.as_ref(),
                on_progress,
                false,
            )
            .await?;
        let (queries_removed, queries_added) =
            llm::recheck::compare_queries(&saved.turn.citations, &after.citations);
        Ok(llm::recheck::Recheck {
            turn_id,
            question: saved.question,
            before: saved.turn,
            after,
            queries_removed,
            queries_added,
        })
    }

    pub fn reference_turns(&self, workspace_id: &str) -> Result<Vec<StoredTurn>> {
        self.db().reference_turns(workspace_id)
    }

    pub fn set_reference_turn(&self, turn_id: i64, reference: bool) -> Result<()> {
        self.db().set_reference_turn(turn_id, reference)
    }

    /// One answered question. `save` keeps it in History and in memory for
    /// follow-ups; re-checks pass false.
    async fn answer(
        &self,
        workspace_id: &str,
        conversation_id: Option<&str>,
        question: &str,
        screen: Option<&llm::context::ScreenContext>,
        on_progress: &(dyn Fn(llm::agent::Progress) + Send + Sync),
        save: bool,
    ) -> Result<Turn> {
        let cfg = self.settings()?.llm;
        let provider = self.llm_provider(&cfg).await?;
        let existing = conversation_id.and_then(|id| {
            self.conversations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id)
        });
        let mut conv = match existing {
            Some(c) => c,
            None => {
                let mut conv = llm::agent::Conversation {
                    id: uuid::Uuid::new_v4().to_string(),
                    messages: Vec::new(),
                    redactor: llm::redact::Redactor::new(
                        cfg.redaction_enabled(),
                        self.redaction_names(workspace_id)?,
                    ),
                    citations: Vec::new(),
                };
                if let Some(id) = conversation_id {
                    self.resume_conversation(&mut conv, workspace_id, id)?;
                }
                conv
            }
        };
        let started = std::time::Instant::now();
        let tools = llm::tools::specs();
        let today = cost::dates::format(cost::dates::today_utc());
        let default_metric = serde_json::to_value(self.settings()?.default_metric)
            .ok()
            .and_then(|v| v.as_str().map(|m| m.replace('_', " ")))
            .unwrap_or_else(|| "unblended".into());
        // The workspace context is written by the user but may name accounts,
        // applications and people: mask it like everything else sent.
        let (plain_context, context_revision) = self.rendered_context(workspace_id)?;
        let context = conv.redactor.mask(&plain_context);
        let screen_text = conv
            .redactor
            .mask(&screen.map(|s| s.render()).unwrap_or_default());
        let system = llm::agent::system_prompt(&llm::agent::PromptContext {
            today: &today,
            redacted: conv.redactor.enabled(),
            default_metric: &default_metric,
            workspace: &context,
            workspace_revision: context_revision,
            screen: &screen_text,
        });
        let conv_id = conv.id.clone();
        let provider_name = format!("{:?}", cfg.provider);
        let redacted = conv.redactor.enabled();
        let result = llm::agent::run_turn(
            provider.as_ref(),
            &tools,
            &mut conv,
            question,
            &system,
            |name, input| llm::tools::execute(self, workspace_id, name, input),
            |reply| {
                let _ = self.db().log_llm_request(
                    workspace_id,
                    &conv_id,
                    &provider_name,
                    &reply.model,
                    redacted,
                    &serde_json::to_string_pretty(&reply.request_json).unwrap_or_default(),
                    &serde_json::to_string_pretty(&reply.response_json).unwrap_or_default(),
                    (reply.input_tokens, reply.output_tokens),
                );
            },
            on_progress,
        )
        .await
        .map(|mut turn| {
            turn.assumptions = llm::tools::assumptions(&turn.citations);
            if !context.is_empty() {
                turn.context_revision = context_revision;
            }
            // Any source of the conversation may back a figure, not only this turn's.
            let results: Vec<&serde_json::Value> =
                conv.citations.iter().map(|c| &c.result).collect();
            turn.unverified_figures =
                llm::grounding::unverified(&turn.answer, &results, &[question, &plain_context]);
            turn.data_as_of = self.data_as_of(workspace_id);
            // A failed save must not lose the answer; it just can't be rated.
            if save {
                turn.turn_id = self
                    .db()
                    .save_assistant_turn(
                        workspace_id,
                        question,
                        screen,
                        &turn,
                        started.elapsed().as_millis() as u64,
                    )
                    .ok();
            }
            turn
        });
        if save {
            self.conversations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(conv.id.clone(), conv);
        }
        result
    }

    /// Rebuilds a saved conversation as plain question/answer history (tool
    /// details are not replayed) so it can continue after a restart.
    fn resume_conversation(
        &self,
        conv: &mut llm::agent::Conversation,
        workspace_id: &str,
        id: &str,
    ) -> Result<()> {
        if self.db().assistant_conversation_workspace(id)?.as_deref() != Some(workspace_id) {
            return Err(Error::NotFound(format!("conversation {id}")));
        }
        conv.id = id.to_owned();
        for saved in self.db().assistant_turns(id)? {
            let question = conv.redactor.mask(&saved.question);
            let answer = conv.redactor.mask(&saved.turn.answer);
            conv.messages.push(llm::Message::user_text(question));
            conv.messages.push(llm::Message {
                role: llm::Role::Assistant,
                blocks: vec![llm::Block::Text(answer)],
            });
            // Keep source numbers increasing across the whole conversation.
            conv.citations.extend(saved.turn.citations);
        }
        Ok(())
    }

    /// Latest successful sync among the workspace's accounts.
    fn data_as_of(&self, workspace_id: &str) -> Option<String> {
        self.db()
            .list_accounts(Some(workspace_id))
            .ok()?
            .into_iter()
            .filter_map(|a| a.sync.and_then(|s| s.last_success_at))
            .max()
    }

    pub fn assistant_conversations(&self, workspace_id: &str) -> Result<Vec<ConversationSummary>> {
        self.db().assistant_conversations(workspace_id, 100)
    }

    pub fn assistant_turns(&self, conversation_id: &str) -> Result<Vec<StoredTurn>> {
        self.db().assistant_turns(conversation_id)
    }

    pub fn rate_assistant_turn(
        &self,
        turn_id: i64,
        rating: Option<Rating>,
        note: Option<&str>,
    ) -> Result<()> {
        self.db().rate_assistant_turn(turn_id, rating, note)
    }

    pub fn delete_assistant_conversation(&self, conversation_id: &str) -> Result<()> {
        self.conversations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(conversation_id);
        self.db().delete_assistant_conversation(conversation_id)
    }

    pub fn llm_log(&self, workspace_id: &str, limit: usize) -> Result<Vec<LlmLogEntry>> {
        self.db().llm_log(workspace_id, limit)
    }

    pub fn clear_llm_log(&self, workspace_id: &str) -> Result<()> {
        self.db().clear_llm_log(workspace_id)
    }
}

/// Patterns and trends for the Insights view (FR-32).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Insights {
    /// Services with the largest change, last 30 days vs the 30 before.
    pub top_movers: Vec<CostComparisonRow>,
    /// Data transfer usage types, last 30 days.
    pub data_transfer: Vec<insights::Labeled>,
    pub data_transfer_total: f64,
    /// Savings Plans / RI coverage per connected account.
    pub commitments: Vec<(String, findings::commitments::CommitmentSummary)>,
    pub unit: String,
}

pub mod insights {
    //! Helpers for the Insights view.

    #[derive(Debug, Clone, PartialEq, serde::Serialize)]
    pub struct Labeled {
        pub key: String,
        pub label: String,
        pub total: f64,
    }

    /// Usage types billed for moving data (Cost Explorer naming).
    pub fn is_data_transfer(usage_type: &str) -> bool {
        usage_type.contains("DataTransfer")
            || usage_type.contains("-Bytes")
            || usage_type.ends_with("Bytes")
    }

    /// `USE1-DataTransfer-Out-Bytes` → `Data transfer out to the internet (USE1)`.
    pub fn describe_usage_type(usage_type: &str) -> String {
        let (region, rest) = match usage_type.split_once('-') {
            Some((r, rest))
                if r.len() <= 5
                    && r.chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
            {
                (Some(r), rest)
            }
            _ => (None, usage_type),
        };
        let what = if rest.starts_with("DataTransfer-Out-Bytes") {
            "Data transfer out to the internet".to_owned()
        } else if rest.starts_with("DataTransfer-In-Bytes") {
            "Data transfer in".to_owned()
        } else if rest.starts_with("DataTransfer-Regional-Bytes") {
            "Data transfer between availability zones".to_owned()
        } else if rest.contains("NatGateway-Bytes") {
            "NAT gateway data processing".to_owned()
        } else if let Some(to) = rest.strip_suffix("-AWS-Out-Bytes") {
            format!("Data transfer to {to}")
        } else if let Some(from) = rest.strip_suffix("-AWS-In-Bytes") {
            format!("Data transfer from {from}")
        } else if rest.contains("CloudFront") || rest.contains("DataTransfer-Out-OBytes") {
            "CloudFront / origin data transfer".to_owned()
        } else {
            rest.replace('-', " ")
        };
        match region {
            Some(r) => format!("{what} ({r})"),
            None => what,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn describes_usage_types() {
            assert!(is_data_transfer("USE1-DataTransfer-Out-Bytes"));
            assert!(is_data_transfer("EUW1-NatGateway-Bytes"));
            assert!(!is_data_transfer("USE1-BoxUsage:m5.large"));
            assert_eq!(
                describe_usage_type("USE1-DataTransfer-Out-Bytes"),
                "Data transfer out to the internet (USE1)"
            );
            assert_eq!(
                describe_usage_type("USE1-EUW1-AWS-Out-Bytes"),
                "Data transfer to EUW1 (USE1)"
            );
            assert_eq!(
                describe_usage_type("USE1-EUW1-AWS-In-Bytes"),
                "Data transfer from EUW1 (USE1)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Collected, Resource, ResourceType, arn};

    fn afinin_with_unattached_volume() -> (Afinin, String) {
        let storage = Storage::open_in_memory().unwrap();
        let ws = storage.create_workspace("W").unwrap();
        let acc = storage
            .add_account(&ws.id, "A", "a", ProfileKind::Sso)
            .unwrap();
        let mut found = Collected::default();
        let mut vol = Resource::new(
            ResourceType::EbsVolume,
            arn::ec2("us-east-1", "1", "volume", "vol-1"),
            "1",
            "us-east-1",
            "vol-1",
        );
        vol.attrs
            .insert("state".into(), serde_json::json!("available"));
        vol.attrs
            .insert("volume_type".into(), serde_json::json!("gp2"));
        vol.attrs.insert("size_gib".into(), serde_json::json!(200));
        found.resources.push(vol);
        storage
            .replace_inventory(&acc.id, "us-east-1", &found)
            .unwrap();
        let afinin = Afinin::with_storage(
            storage,
            Arc::new(MemoryStore::default()),
            AwsFiles::from_env(),
        );
        (afinin, ws.id)
    }

    #[test]
    fn finding_lifecycle() {
        let (afinin, ws) = afinin_with_unattached_volume();
        let open = afinin.findings(&ws, false).unwrap();
        assert_eq!(open.len(), 1);
        let f = &open[0];
        assert_eq!(f.rule, "ebs-unattached");
        assert_eq!(f.monthly_savings, Some(20.0)); // 200 GiB × $0.10 gp2
        assert_eq!(
            f.first_seen.as_deref(),
            Some(cost::dates::format(cost::dates::today_utc()).as_str())
        );

        afinin
            .set_finding_status(
                &f.id,
                &FindingStatus::Dismissed {
                    note: Some("keep".into()),
                },
            )
            .unwrap();
        assert!(afinin.findings(&ws, false).unwrap().is_empty());
        assert_eq!(afinin.findings(&ws, true).unwrap().len(), 1);

        // A snooze in the past has expired: the finding is open again.
        afinin
            .set_finding_status(
                &f.id,
                &FindingStatus::Snoozed {
                    until: "2000-01-01".into(),
                },
            )
            .unwrap();
        assert_eq!(afinin.findings(&ws, false).unwrap().len(), 1);
        afinin
            .set_finding_status(
                &f.id,
                &FindingStatus::Snoozed {
                    until: "2999-01-01".into(),
                },
            )
            .unwrap();
        assert!(afinin.findings(&ws, false).unwrap().is_empty());
        afinin
            .set_finding_status(&f.id, &FindingStatus::Open)
            .unwrap();
        assert_eq!(afinin.findings(&ws, false).unwrap().len(), 1);
    }

    #[test]
    fn assistant_tools_read_local_data() {
        let (afinin, ws) = afinin_with_unattached_volume();
        let findings =
            llm::tools::execute(&afinin, &ws, "list_findings", &serde_json::json!({})).unwrap();
        assert_eq!(findings["total_monthly_savings"], 20.0);
        let found = llm::tools::execute(
            &afinin,
            &ws,
            "search_resources",
            &serde_json::json!({"query": "vol-1"}),
        )
        .unwrap();
        assert_eq!(found["matches"][0]["type"], "ec2:volume");
        let arn = found["matches"][0]["arn"].as_str().unwrap().to_owned();
        let res = llm::tools::execute(
            &afinin,
            &ws,
            "get_resource",
            &serde_json::json!({"arn": arn}),
        )
        .unwrap();
        assert_eq!(res["attributes"]["size_gib"], 200);
        assert!(llm::tools::execute(&afinin, &ws, "nope", &serde_json::json!({})).is_err());
        // Every tool has an object schema.
        assert!(
            llm::tools::specs()
                .iter()
                .all(|t| t.schema["type"] == "object")
        );
    }
}
