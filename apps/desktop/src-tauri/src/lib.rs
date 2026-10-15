// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tauri shell. Commands are thin wrappers over `afinin-core`; keep this IPC
//! surface minimal (NFR-5).

use afinin_core::cur::template;
use afinin_core::llm::agent::Progress;
use afinin_core::llm::recheck::Recheck;
use afinin_core::{
    Account, Afinin, ApiUsage, AppPaths, AwsFiles, AwsProfile, CostComparisonRow, CostMetric,
    CostOverview, CostQuery, CostSeries, DimensionValue, Error, GroupBy, KeychainStore, Settings,
    SsoLoginPrompt, SsoLoginStatus, Workspace,
};
use afinin_core::{
    AccountSyncReport, Application, Facet, Graph, GraphFilter, InventoryFacets, InventoryScan,
    Resource,
};
use afinin_core::{
    ContextRevision, ConversationSummary, CurConfig, CurStatus, CurValidation, Finding,
    FindingStatus, Insights, LlmLogEntry, LlmUsage, Rating, ScreenContext, StoredContext,
    StoredTurn, Turn, WorkspaceContext,
};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

/// Event telling the UI that an account's sync status or data changed.
const SYNC_EVENT: &str = "sync-changed";
/// Payload: `{ workspace_id, stage, … }` while the assistant works on a turn.
const ASSISTANT_PROGRESS_EVENT: &str = "assistant-progress";
/// How often the background scheduler looks for accounts due for sync.
const SCHEDULER_TICK: Duration = Duration::from_secs(10 * 60);

type CmdResult<T> = Result<T, Error>;

/// The core may fail to open (e.g. no keychain available); the UI shows why.
struct AppState(Result<Arc<Afinin>, String>);

impl AppState {
    fn core(&self) -> CmdResult<&Afinin> {
        self.0.as_deref().map_err(|e| Error::Other(e.clone()))
    }
}

/// Runs a command's work on a blocking thread. Synchronous commands run on
/// the main thread, where any slow (or stuck) database or file access
/// freezes the window ("Not responding").
async fn blocking<T: Send + 'static>(
    state: &State<'_, AppState>,
    work: impl FnOnce(&Afinin) -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    let core = state
        .0
        .as_ref()
        .map_err(|e| Error::Other(e.clone()))?
        .clone();
    tauri::async_runtime::spawn_blocking(move || work(&core))
        .await
        .map_err(|e| Error::Other(format!("background task failed: {e}")))?
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
async fn list_workspaces(state: State<'_, AppState>) -> CmdResult<Vec<Workspace>> {
    blocking(&state, move |core| core.list_workspaces()).await
}

#[tauri::command]
async fn create_workspace(state: State<'_, AppState>, name: String) -> CmdResult<Workspace> {
    blocking(&state, move |core| core.create_workspace(&name)).await
}

#[tauri::command]
async fn rename_workspace(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    blocking(&state, move |core| core.rename_workspace(&id, &name)).await
}

#[tauri::command]
async fn delete_workspace(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    blocking(&state, move |core| core.delete_workspace(&id)).await
}

#[tauri::command]
async fn list_accounts(
    state: State<'_, AppState>,
    workspace_id: Option<String>,
) -> CmdResult<Vec<Account>> {
    blocking(&state, move |core| {
        core.list_accounts(workspace_id.as_deref())
    })
    .await
}

#[tauri::command]
async fn add_account(
    state: State<'_, AppState>,
    workspace_id: String,
    profile: String,
    name: Option<String>,
) -> CmdResult<Account> {
    blocking(&state, move |core| {
        core.add_account(&workspace_id, &profile, name.as_deref())
    })
    .await
}

#[tauri::command]
async fn remove_account(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    blocking(&state, move |core| core.remove_account(&id)).await
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
async fn sso_logout(state: State<'_, AppState>, profile: String) -> CmdResult<()> {
    blocking(&state, move |core| core.sso_logout(&profile)).await
}

// ---- settings ------------------------------------------------------------

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    blocking(&state, move |core| core.settings()).await
}

#[tauri::command]
async fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    blocking(&state, move |core| core.save_settings(&settings)).await
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
async fn api_usage(state: State<'_, AppState>) -> CmdResult<Vec<ApiUsage>> {
    blocking(&state, move |core| core.api_usage_this_month()).await
}

#[tauri::command]
async fn llm_usage(state: State<'_, AppState>) -> CmdResult<Vec<LlmUsage>> {
    blocking(&state, move |core| core.llm_usage_this_month()).await
}

// ---- costs ---------------------------------------------------------------

#[tauri::command]
async fn cost_overview(
    state: State<'_, AppState>,
    workspace_id: String,
    metric: CostMetric,
    linked_accounts: Vec<String>,
) -> CmdResult<CostOverview> {
    blocking(&state, move |core| {
        core.cost_overview(&workspace_id, metric, &linked_accounts)
    })
    .await
}

#[tauri::command]
async fn cost_series(state: State<'_, AppState>, query: CostQuery) -> CmdResult<CostSeries> {
    blocking(&state, move |core| core.cost_series(&query)).await
}

#[tauri::command]
async fn compare_costs(
    state: State<'_, AppState>,
    query: CostQuery,
    prev_start: String,
    prev_end: String,
) -> CmdResult<Vec<CostComparisonRow>> {
    blocking(&state, move |core| {
        core.compare_costs(&query, &prev_start, &prev_end)
    })
    .await
}

#[tauri::command]
async fn dimension_values(
    state: State<'_, AppState>,
    workspace_id: String,
    group_by: GroupBy,
) -> CmdResult<Vec<DimensionValue>> {
    blocking(&state, move |core| {
        core.dimension_values(&workspace_id, &group_by)
    })
    .await
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
async fn inventory_graph(
    state: State<'_, AppState>,
    workspace_id: String,
    filter: GraphFilter,
) -> CmdResult<Graph> {
    blocking(&state, move |core| {
        core.inventory_graph(&workspace_id, &filter)
    })
    .await
}

#[tauri::command]
async fn inventory_facets(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<InventoryFacets> {
    blocking(&state, move |core| core.inventory_facets(&workspace_id)).await
}

#[tauri::command]
async fn tag_values(
    state: State<'_, AppState>,
    workspace_id: String,
    key: String,
) -> CmdResult<Vec<Facet>> {
    blocking(&state, move |core| core.tag_values(&workspace_id, &key)).await
}

#[tauri::command]
async fn applications(
    state: State<'_, AppState>,
    workspace_id: String,
    tag_key: String,
) -> CmdResult<Vec<Application>> {
    blocking(&state, move |core| {
        core.applications(&workspace_id, &tag_key)
    })
    .await
}

#[tauri::command]
async fn resource_neighbors(
    state: State<'_, AppState>,
    workspace_id: String,
    arn: String,
    depth: usize,
) -> CmdResult<Vec<(Resource, usize)>> {
    blocking(&state, move |core| {
        core.resource_neighbors(&workspace_id, &arn, depth.min(5))
    })
    .await
}

#[tauri::command]
async fn inventory_scans(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<Vec<InventoryScan>> {
    blocking(&state, move |core| core.inventory_scans(&workspace_id)).await
}

// ---- CUR -------------------------------------------------------------------

#[tauri::command]
async fn validate_cur(state: State<'_, AppState>, config: CurConfig) -> CmdResult<CurValidation> {
    state.core()?.validate_cur(&config).await
}

#[tauri::command]
async fn save_cur_config(state: State<'_, AppState>, config: CurConfig) -> CmdResult<()> {
    blocking(&state, move |core| core.save_cur_config(&config)).await
}

#[tauri::command]
async fn get_cur_config(
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Option<CurConfig>> {
    blocking(&state, move |core| core.cur_config(&account_id)).await
}

#[tauri::command]
async fn delete_cur_config(state: State<'_, AppState>, account_id: String) -> CmdResult<()> {
    blocking(&state, move |core| core.delete_cur_config(&account_id)).await
}

#[tauri::command]
async fn cur_status(state: State<'_, AppState>, workspace_id: String) -> CmdResult<CurStatus> {
    blocking(&state, move |core| core.cur_status(&workspace_id)).await
}

#[derive(serde::Serialize)]
struct CurSetupHelp {
    deploy_command: String,
    read_policy: String,
}

#[tauri::command]
fn cur_setup_help(bucket: String) -> CurSetupHelp {
    CurSetupHelp {
        deploy_command: template::deploy_command(&bucket),
        read_policy: template::read_policy(&bucket),
    }
}

/// Saves the CloudFormation template where the user chooses.
#[tauri::command]
async fn save_cur_template(app: AppHandle) -> CmdResult<bool> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("CloudFormation", &["yaml", "yml"])
        .set_file_name("afinin-cur-export.yaml")
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(path, template::CLOUDFORMATION)?;
    Ok(true)
}

// ---- findings ----------------------------------------------------------------

#[tauri::command]
async fn findings(
    state: State<'_, AppState>,
    workspace_id: String,
    include_hidden: bool,
) -> CmdResult<Vec<Finding>> {
    blocking(&state, move |core| {
        core.findings(&workspace_id, include_hidden)
    })
    .await
}

#[tauri::command]
async fn set_finding_status(
    state: State<'_, AppState>,
    finding_id: String,
    status: FindingStatus,
) -> CmdResult<()> {
    blocking(&state, move |core| {
        core.set_finding_status(&finding_id, &status)
    })
    .await
}

#[tauri::command]
async fn insights(state: State<'_, AppState>, workspace_id: String) -> CmdResult<Insights> {
    blocking(&state, move |core| core.insights(&workspace_id)).await
}

// ---- assistant -----------------------------------------------------------------

#[tauri::command]
async fn ask(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    conversation_id: Option<String>,
    question: String,
    screen: Option<ScreenContext>,
) -> CmdResult<Turn> {
    let progress = |p: Progress| {
        let mut payload = serde_json::to_value(&p).unwrap_or_default();
        payload["workspace_id"] = workspace_id.clone().into();
        let _ = app.emit(ASSISTANT_PROGRESS_EVENT, payload);
    };
    state
        .core()?
        .ask(
            &workspace_id,
            conversation_id.as_deref(),
            &question,
            screen.as_ref(),
            &progress,
        )
        .await
}

#[tauri::command]
async fn recheck_reference(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    turn_id: i64,
) -> CmdResult<Recheck> {
    let progress = |p: Progress| {
        let mut payload = serde_json::to_value(&p).unwrap_or_default();
        payload["workspace_id"] = workspace_id.clone().into();
        payload["recheck_turn_id"] = turn_id.into();
        let _ = app.emit(ASSISTANT_PROGRESS_EVENT, payload);
    };
    state
        .core()?
        .recheck_reference(&workspace_id, turn_id, &progress)
        .await
}

#[tauri::command]
async fn assistant_gaps(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<Vec<StoredTurn>> {
    blocking(&state, move |core| core.assistant_gaps(&workspace_id)).await
}

#[tauri::command]
async fn reference_turns(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<Vec<StoredTurn>> {
    blocking(&state, move |core| core.reference_turns(&workspace_id)).await
}

#[tauri::command]
async fn set_reference_turn(
    state: State<'_, AppState>,
    turn_id: i64,
    reference: bool,
) -> CmdResult<()> {
    blocking(&state, move |core| {
        core.set_reference_turn(turn_id, reference)
    })
    .await
}

#[tauri::command]
async fn set_reference_expected(
    state: State<'_, AppState>,
    turn_id: i64,
    expected: Vec<String>,
) -> CmdResult<Vec<String>> {
    blocking(&state, move |core| {
        core.set_reference_expected(turn_id, &expected)
    })
    .await
}

#[tauri::command]
async fn reference_figures(state: State<'_, AppState>, turn_id: i64) -> CmdResult<Vec<String>> {
    blocking(&state, move |core| core.reference_figures(turn_id)).await
}

#[tauri::command]
async fn assistant_conversations(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<Vec<ConversationSummary>> {
    blocking(&state, move |core| {
        core.assistant_conversations(&workspace_id)
    })
    .await
}

#[tauri::command]
async fn assistant_turns(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<Vec<StoredTurn>> {
    blocking(&state, move |core| core.assistant_turns(&conversation_id)).await
}

#[tauri::command]
async fn rate_assistant_turn(
    state: State<'_, AppState>,
    turn_id: i64,
    rating: Option<Rating>,
    note: Option<String>,
) -> CmdResult<()> {
    blocking(&state, move |core| {
        core.rate_assistant_turn(turn_id, rating, note.as_deref())
    })
    .await
}

#[tauri::command]
async fn delete_assistant_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> CmdResult<()> {
    blocking(&state, move |core| {
        core.delete_assistant_conversation(&conversation_id)
    })
    .await
}

#[tauri::command]
async fn workspace_context(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<StoredContext> {
    blocking(&state, move |core| core.workspace_context(&workspace_id)).await
}

#[tauri::command]
async fn save_workspace_context(
    state: State<'_, AppState>,
    workspace_id: String,
    context: WorkspaceContext,
) -> CmdResult<StoredContext> {
    blocking(&state, move |core| {
        core.save_workspace_context(&workspace_id, context)
    })
    .await
}

#[tauri::command]
async fn workspace_context_history(
    state: State<'_, AppState>,
    workspace_id: String,
) -> CmdResult<Vec<ContextRevision>> {
    blocking(&state, move |core| {
        core.workspace_context_history(&workspace_id)
    })
    .await
}

/// Stores (or with `None`, removes) the provider API key in the OS keychain.
#[tauri::command]
async fn set_llm_api_key(state: State<'_, AppState>, key: Option<String>) -> CmdResult<()> {
    blocking(&state, move |core| core.set_llm_api_key(key.as_deref())).await
}

#[tauri::command]
async fn has_llm_api_key(state: State<'_, AppState>) -> CmdResult<bool> {
    blocking(&state, move |core| core.has_llm_api_key()).await
}

#[tauri::command]
async fn llm_log(state: State<'_, AppState>, workspace_id: String) -> CmdResult<Vec<LlmLogEntry>> {
    blocking(&state, move |core| core.llm_log(&workspace_id, 200)).await
}

#[tauri::command]
async fn llm_log_entry(
    state: State<'_, AppState>,
    workspace_id: String,
    id: i64,
) -> CmdResult<LlmLogEntry> {
    blocking(&state, move |core| core.llm_log_entry(&workspace_id, id)).await
}

#[tauri::command]
async fn clear_llm_log(state: State<'_, AppState>, workspace_id: String) -> CmdResult<()> {
    blocking(&state, move |core| core.clear_llm_log(&workspace_id)).await
}

// ---- backup (FR-53) -----------------------------------------------------------

/// Asks where to save, then writes a password-encrypted backup. `false` if cancelled.
#[tauri::command]
async fn export_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    password: String,
) -> CmdResult<bool> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Afinín backup", &["afinin"])
        .set_file_name("afinin-backup.afinin")
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| Error::Other(e.to_string()))?;
    state.core()?.export_backup(&path, &password)?;
    Ok(true)
}

/// Asks for a backup file and replaces all local data with it. `false` if cancelled.
#[tauri::command]
async fn import_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    password: String,
) -> CmdResult<bool> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Afinín backup", &["afinin"])
        .blocking_pick_file()
    else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| Error::Other(e.to_string()))?;
    state.core()?.import_backup(&path, &password)?;
    Ok(true)
}

// ---- updates (NFR-6) ---------------------------------------------------------

#[derive(serde::Serialize)]
struct UpdateInfo {
    version: String,
    notes: Option<String>,
}

/// Checks for a signed update. Always `None` in builds without the updater.
#[tauri::command]
async fn check_update(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    #[cfg(feature = "updater")]
    {
        use tauri_plugin_updater::UpdaterExt;
        let updater = app.updater().map_err(|e| Error::Other(e.to_string()))?;
        let update = updater
            .check()
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok(update.map(|u| UpdateInfo {
            version: u.version,
            notes: u.body,
        }))
    }
    #[cfg(not(feature = "updater"))]
    {
        let _ = app;
        Ok(None)
    }
}

/// Downloads, verifies (signature) and installs the update, then restarts.
#[tauri::command]
async fn install_update(app: AppHandle) -> CmdResult<()> {
    #[cfg(feature = "updater")]
    {
        use tauri_plugin_updater::UpdaterExt;
        let updater = app.updater().map_err(|e| Error::Other(e.to_string()))?;
        if let Some(update) = updater
            .check()
            .await
            .map_err(|e| Error::Other(e.to_string()))?
        {
            update
                .download_and_install(|_, _| {}, || {})
                .await
                .map_err(|e| Error::Other(e.to_string()))?;
            app.restart();
        }
        Ok(())
    }
    #[cfg(not(feature = "updater"))]
    {
        let _ = app;
        Err(Error::Other("this build has no auto-updater".into()))
    }
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

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(feature = "updater")]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .manage(AppState(core.map(Arc::new)))
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
            validate_cur,
            save_cur_config,
            get_cur_config,
            delete_cur_config,
            cur_status,
            cur_setup_help,
            save_cur_template,
            findings,
            set_finding_status,
            insights,
            ask,
            set_llm_api_key,
            has_llm_api_key,
            llm_log,
            llm_log_entry,
            clear_llm_log,
            assistant_conversations,
            assistant_turns,
            recheck_reference,
            assistant_gaps,
            llm_usage,
            reference_turns,
            set_reference_turn,
            set_reference_expected,
            reference_figures,
            rate_assistant_turn,
            delete_assistant_conversation,
            workspace_context,
            save_workspace_context,
            workspace_context_history,
            export_backup,
            import_backup,
            check_update,
            install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Afinín");
}
