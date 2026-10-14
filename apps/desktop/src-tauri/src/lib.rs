// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tauri shell. Commands are thin wrappers over `afinin-core`; keep this IPC
//! surface minimal (NFR-5).

use afinin_core::{
    Account, Afinin, AppPaths, AwsFiles, AwsProfile, Error, KeychainStore, SsoLoginPrompt,
    SsoLoginStatus, Workspace,
};
use std::sync::Arc;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

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
        .manage(AppState(core))
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Afinín");
}
