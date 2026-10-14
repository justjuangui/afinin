// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tauri shell. Commands are thin wrappers over `afinin-core`; keep this IPC
//! surface minimal (NFR-5).

use afinin_core::{
    Account, Afinin, ApiUsage, AppPaths, AwsFiles, AwsProfile, CostComparisonRow, CostMetric,
    CostOverview, CostQuery, CostSeries, DimensionValue, Error, GroupBy, KeychainStore, Settings,
    SsoLoginPrompt, SsoLoginStatus, Workspace,
};
use afinin_core::{
    AccountSyncReport, Application, Facet, Graph, GraphFilter, InventoryFacets, InventoryScan,
    Resource,
};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

/// Event telling the UI that an account's sync status or data changed.
const SYNC_EVENT: &str = "sync-changed";
/// How often the background scheduler looks for accounts due for sync.
const SCHEDULER_TICK: Duration = Duration::from_secs(10 * 60);

type CmdResult<T> = Result<T, Error>;

/// The core may fail to open (e.g. no keychain available); the UI shows why.
struct AppState(Result<Afinin, String>);

impl AppState {
    fn core(&self) -> CmdResult<&Afinin> {
        self.0.as_ref().map_err(|e| Error::Other(e.clone()))
    }
}

fn open_core() -> Result<Afinin, String> {
    let paths = AppPaths::platform_default().map_err(|e| e.to_string())?;
    Afinin::open(&paths, Arc::new(KeychainStore), AwsFiles::from_env()).map_err(|e| e.to_string())
}

#[tauri::command]
fn startup_error(state: State<'_, AppState>) -> Option<String> {
    state.0.as_ref().err().cloned()
}

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> CmdResult<Vec<AwsProfile>> {
    state.core()?.list_profiles()
}

#[tauri::command]
fn list_workspaces(state: State<'_, AppState>) -> CmdResult<Vec<Workspace>> {
    state.core()?.list_workspaces()
}

#[tauri::command]
fn create_workspace(state: State<'_, AppState>, name: String) -> CmdResult<Workspace> {
    state.core()?.create_workspace(&name)
}

#[tauri::command]
fn rename_workspace(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    state.core()?.rename_workspace(&id, &name)
}

#[tauri::command]
fn delete_workspace(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core()?.delete_workspace(&id)
}

#[tauri::command]
fn list_accounts(
    state: State<'_, AppState>,
    workspace_id: Option<String>,
) -> CmdResult<Vec<Account>> {
    state.core()?.list_accounts(workspace_id.as_deref())
}

#[tauri::command]
fn add_account(
    state: State<'_, AppState>,
    workspace_id: String,
    profile: String,
    name: Option<String>,
) -> CmdResult<Account> {
    state
        .core()?
        .add_account(&workspace_id, &profile, name.as_deref())
}

#[tauri::command]
fn remove_account(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core()?.remove_account(&id)
}

#[tauri::command]
async fn test_connection(state: State<'_, AppState>, id: String) -> CmdResult<Account> {
    state.core()?.test_connection(&id).await
}

/// Starts SSO login and opens the AWS approval page in the system browser.
/// Opening happens here so the webview needs no URL-opening permission.
#[tauri::command]
async fn start_sso_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    profile: String,
) -> CmdResult<SsoLoginPrompt> {
    let prompt = state.core()?.start_sso_login(&profile).await?;
    let url = prompt
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&prompt.verification_uri);
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        // The UI still shows the URL and code, so the user can open it manually.
        tracing::warn!("could not open browser: {e}");
    }
    Ok(prompt)
}

#[tauri::command]
async fn poll_sso_login(state: State<'_, AppState>, profile: String) -> CmdResult<SsoLoginStatus> {
    state.core()?.poll_sso_login(&profile).await
}

/// Documentation links the UI may open in the system browser.
const DOCS_PREFIXES: &[&str] = &["https://docs.aws.amazon.com/"];

#[tauri::command]
fn open_docs(app: tauri::AppHandle, url: String) -> CmdResult<()> {
    if !DOCS_PREFIXES.iter().any(|p| url.starts_with(p)) {
        return Err(Error::Other("URL not allowed".into()));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| Error::Other(e.to_string()))
}

#[tauri::command]
fn sso_logout(state: State<'_, AppState>, profile: String) -> CmdResult<()> {
    state.core()?.sso_logout(&profile)
}

// ---- settings ------------------------------------------------------------

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    state.core()?.settings()
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    state.core()?.save_settings(&settings)
}

// ---- sync ----------------------------------------------------------------

async fn run_sync(
    app: &AppHandle,
    core: &Afinin,
    account_id: &str,
) -> CmdResult<AccountSyncReport> {
    let _ = app.emit(SYNC_EVENT, account_id);
    let result = core.sync_account(account_id).await;
    let _ = app.emit(SYNC_EVENT, account_id);
    result
}

#[tauri::command]
async fn sync_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<AccountSyncReport> {
    run_sync(&app, state.core()?, &id).await
}

/// Syncs every account of a workspace in turn. Failures are reported per
/// account through its sync status, so one bad account doesn't stop the rest.
#[tauri::command]
async fn sync_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<()> {
    let core = state.core()?;
    for account in core.list_accounts(Some(&workspace_id))? {
        let _ = run_sync(&app, core, &account.id).await;
    }
    Ok(())
}

/// Background sync on the configured schedule (FR-50).
fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<AppState>();
            if let Ok(core) = state.core() {
                match core.accounts_due_for_sync() {
                    Ok(due) => {
                        for id in due {
                            let _ = run_sync(&app, core, &id).await;
                        }
                    }
                    Err(e) => tracing::warn!("scheduler: {e}"),
                }
            }
            tokio::time::sleep(SCHEDULER_TICK).await;
        }
    });
}

#[tauri::command]
fn api_usage(state: State<'_, AppState>) -> CmdResult<Vec<ApiUsage>> {
    state.core()?.api_usage_this_month()
}

// ---- costs ---------------------------------------------------------------

#[tauri::command]
fn cost_overview(
    state: State<'_, AppState>,
    workspace_id: String,
    metric: CostMetric,
    linked_accounts: Vec<String>,
) -> CmdResult<CostOverview> {
    state
        .core()?
        .cost_overview(&workspace_id, metric, &linked_accounts)
}

#[tauri::command]
fn cost_series(state: State<'_, AppState>, query: CostQuery) -> CmdResult<CostSeries> {
    state.core()?.cost_series(&query)
}

#[tauri::command]
fn compare_costs(
    state: State<'_, AppState>,
    query: CostQuery,
    prev_start: String,
    prev_end: String,
) -> CmdResult<Vec<CostComparisonRow>> {
    state.core()?.compare_costs(&query, &prev_start, &prev_end)
}

#[tauri::command]
fn dimension_values(
    state: State<'_, AppState>,
    workspace_id: String,
    group_by: GroupBy,
) -> CmdResult<Vec<DimensionValue>> {
    state.core()?.dimension_values(&workspace_id, &group_by)
}

/// Asks where to save and writes the CSV (FR-16). Returns `false` if cancelled.
/// The file dialog runs on the Rust side so the webview needs no fs access.
#[tauri::command]
async fn export_csv(
    app: AppHandle,
    state: State<'_, AppState>,
    query: CostQuery,
    compare: Option<(String, String)>,
    file_name: String,
) -> CmdResult<bool> {
    let core = state.core()?;
    let csv = match &compare {
        Some((start, end)) => core.export_comparison_csv(&query, start, end)?,
        None => core.export_series_csv(&query)?,
    };
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("CSV", &["csv"])
        .set_file_name(&file_name)
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(path, csv)?;
    Ok(true)
}

// ---- inventory -------------------------------------------------------------

#[tauri::command]
fn inventory_graph(
    state: State<'_, AppState>,
    workspace_id: String,
    filter: GraphFilter,
) -> CmdResult<Graph> {
    state.core()?.inventory_graph(&workspace_id, &filter)
}

#[tauri::command]
fn inventory_facets(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<InventoryFacets> {
    state.core()?.inventory_facets(&workspace_id)
}

#[tauri::command]
fn tag_values(
    state: State<'_, AppState>,
    workspace_id: String,
    key: String,
) -> CmdResult<Vec<Facet>> {
    state.core()?.tag_values(&workspace_id, &key)
}

#[tauri::command]
fn applications(
    state: State<'_, AppState>,
    workspace_id: String,
    tag_key: String,
) -> CmdResult<Vec<Application>> {
    state.core()?.applications(&workspace_id, &tag_key)
}

#[tauri::command]
fn resource_neighbors(
    state: State<'_, AppState>,
    workspace_id: String,
    arn: String,
    depth: usize,
) -> CmdResult<Vec<(Resource, usize)>> {
    state
        .core()?
        .resource_neighbors(&workspace_id, &arn, depth.min(5))
}

#[tauri::command]
fn inventory_scans(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<Vec<InventoryScan>> {
    state.core()?.inventory_scans(&workspace_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let core = open_core();
    if let Err(e) = &core {
        tracing::error!("failed to open local data: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState(core))
        .setup(|app| {
            spawn_scheduler(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            startup_error,
            list_profiles,
            list_workspaces,
            create_workspace,
            rename_workspace,
            delete_workspace,
            list_accounts,
            add_account,
            remove_account,
            test_connection,
            start_sso_login,
            poll_sso_login,
            sso_logout,
            open_docs,
            get_settings,
            save_settings,
            sync_account,
            sync_workspace,
            api_usage,
            cost_overview,
            cost_series,
            compare_costs,
            dimension_values,
            export_csv,
            inventory_graph,
            inventory_facets,
            tag_values,
            applications,
            resource_neighbors,
            inventory_scans,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Afinín");
}
