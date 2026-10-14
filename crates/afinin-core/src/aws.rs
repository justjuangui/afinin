// SPDX-License-Identifier: MIT OR Apache-2.0

//! AWS credentials and identity (FR-1, FR-3). Only read-only API calls are made.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_credential_types::Credentials;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{ConnectionTest, now_rfc3339};
use crate::profiles::{AwsProfile, ProfileKind, SsoSettings};
use crate::secrets::SecretStore;

/// Region used for global endpoints when a profile does not set one.
const FALLBACK_REGION: &str = "us-east-1";

/// SSO access token obtained through the in-app device login, kept in the keychain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SsoToken {
    pub access_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

impl SsoToken {
    fn is_valid(&self) -> bool {
        // Keep a margin so we never start a call with a token about to expire.
        unix_now() + 60 < self.expires_at
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

pub(crate) fn sso_secret_key(sso: &SsoSettings) -> String {
    format!("sso-token:{}", sso.token_key())
}

pub(crate) fn load_sso_token(
    secrets: &dyn SecretStore,
    sso: &SsoSettings,
) -> Result<Option<SsoToken>> {
    let Some(raw) = secrets.get(&sso_secret_key(sso))? else {
        return Ok(None);
    };
    // A corrupt entry is treated as "not logged in" rather than a hard error.
    Ok(serde_json::from_str::<SsoToken>(&raw)
        .ok()
        .filter(SsoToken::is_valid))
}

pub(crate) fn store_sso_token(
    secrets: &dyn SecretStore,
    sso: &SsoSettings,
    token: &SsoToken,
) -> Result<()> {
    let raw = serde_json::to_string(token).map_err(|e| Error::Other(e.to_string()))?;
    secrets.set(&sso_secret_key(sso), &raw)
}

/// Builds the SDK configuration for a profile.
///
/// SSO profiles use the token from the in-app login when present; otherwise the
/// standard AWS SDK chain applies (including tokens cached by `aws sso login`).
pub async fn sdk_config(profile: &AwsProfile, secrets: &dyn SecretStore) -> Result<SdkConfig> {
    let mut loader = aws_config::defaults(BehaviorVersion::latest()).profile_name(&profile.name);
    if profile.region.is_none() {
        let region = profile
            .sso
            .as_ref()
            .map(|s| s.region.clone())
            .unwrap_or_else(|| FALLBACK_REGION.to_owned());
        loader = loader.region(Region::new(region));
    }
    if profile.kind == ProfileKind::Sso {
        let sso = profile.sso.as_ref().expect("SSO profile has SSO settings");
        if let Some(token) = load_sso_token(secrets, sso)? {
            loader = loader.credentials_provider(sso_role_credentials(profile, sso, &token).await?);
        }
    }
    Ok(loader.load().await)
}

async fn sso_role_credentials(
    profile: &AwsProfile,
    sso: &SsoSettings,
    token: &SsoToken,
) -> Result<Credentials> {
    let (Some(account_id), Some(role_name)) = (&sso.account_id, &sso.role_name) else {
        return Err(Error::InvalidProfile(format!(
            "profile `{}` must set `sso_account_id` and `sso_role_name`",
            profile.name
        )));
    };
    let client = aws_sdk_sso::Client::from_conf(
        aws_sdk_sso::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(sso.region.clone()))
            .build(),
    );
    let out = client
        .get_role_credentials()
        .account_id(account_id)
        .role_name(role_name)
        .access_token(&token.access_token)
        .send()
        .await
        .map_err(Error::aws)?;
    let rc = out
        .role_credentials()
        .ok_or_else(|| Error::Aws("SSO returned no role credentials".into()))?;
    let field = |v: Option<&str>, name: &str| {
        v.map(str::to_owned)
            .ok_or_else(|| Error::Aws(format!("SSO role credentials missing {name}")))
    };
    let expiry = UNIX_EPOCH + Duration::from_millis(rc.expiration().max(0) as u64);
    Ok(Credentials::new(
        field(rc.access_key_id(), "access key")?,
        field(rc.secret_access_key(), "secret key")?,
        rc.session_token().map(str::to_owned),
        Some(expiry),
        "afinin-sso",
    ))
}

/// Validates identity with `sts:GetCallerIdentity` (FR-3).
///
/// Failures are reported inside the returned [`ConnectionTest`], not as errors,
/// so the UI can show them next to the account.
pub async fn test_connection(profile: &AwsProfile, secrets: &dyn SecretStore) -> ConnectionTest {
    let result = async {
        let config = sdk_config(profile, secrets).await?;
        let identity = aws_sdk_sts::Client::new(&config)
            .get_caller_identity()
            .send()
            .await
            .map_err(Error::aws)?;
        let missing = crate::permissions::missing_permissions(&config).await;
        Ok::<_, Error>((identity, missing))
    }
    .await;

    match result {
        Ok((out, missing)) => ConnectionTest {
            ok: true,
            tested_at: now_rfc3339(),
            aws_account_id: out.account().map(str::to_owned),
            arn: out.arn().map(str::to_owned),
            error: None,
            missing_permissions: missing.into_iter().map(str::to_owned).collect(),
        },
        Err(e) => ConnectionTest {
            ok: false,
            tested_at: now_rfc3339(),
            aws_account_id: None,
            arn: None,
            error: Some(e.to_string()),
            missing_permissions: Vec::new(),
        },
    }
}
