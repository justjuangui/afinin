// SPDX-License-Identifier: MIT OR Apache-2.0

//! Afinín core: read-only AWS cost and inventory analysis.
//!
//! This crate is independent of Tauri so a CLI or TUI can reuse it.

pub mod aws;
pub mod error;
pub mod model;
pub mod profiles;
pub mod secrets;
pub mod sso;
pub mod storage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use error::{Error, Result};
pub use model::{Account, ConnectionTest, Workspace};
pub use profiles::{AwsFiles, AwsProfile, ProfileKind};
pub use secrets::{KeychainStore, MemoryStore, SecretStore};
pub use sso::{SsoLoginPrompt, SsoLoginStatus};
pub use storage::Storage;

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
}
