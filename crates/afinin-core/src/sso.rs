// SPDX-License-Identifier: MIT OR Apache-2.0

//! In-app IAM Identity Center login using the OAuth device authorization flow.
//! The resulting access token is stored in the OS keychain (FR-2), not in the
//! AWS CLI cache directory.

use aws_config::{BehaviorVersion, Region};
use aws_sdk_ssooidc::Client;
use serde::Serialize;

use crate::aws::{SsoToken, store_sso_token, unix_now};
use crate::error::{Error, Result};
use crate::profiles::SsoSettings;
use crate::secrets::SecretStore;

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// What the user must do in the browser to approve the login.
#[derive(Debug, Clone, Serialize)]
pub struct SsoLoginPrompt {
    pub verification_uri: String,
    /// URL with the user code pre-filled, when provided by AWS.
    pub verification_uri_complete: Option<String>,
    pub user_code: String,
    /// Seconds until the device code expires.
    pub expires_in: i32,
    /// Minimum seconds between polls.
    pub interval: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SsoLoginStatus {
    Pending,
    Complete,
    Expired,
}

/// A started login waiting for user approval.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    sso: SsoSettings,
    client_id: String,
    client_secret: String,
    device_code: String,
    expires_at: u64,
}

fn oidc_client(region: &str) -> Client {
    Client::from_conf(
        aws_sdk_ssooidc::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.to_owned()))
            .build(),
    )
}

pub async fn start(sso: &SsoSettings) -> Result<(PendingLogin, SsoLoginPrompt)> {
    let client = oidc_client(&sso.region);
    let reg = client
        .register_client()
        .client_name("afinin")
        .client_type("public")
        .send()
        .await
        .map_err(Error::aws)?;
    let client_id = reg
        .client_id()
        .ok_or_else(|| Error::Aws("SSO OIDC returned no client id".into()))?
        .to_owned();
    let client_secret = reg
        .client_secret()
        .ok_or_else(|| Error::Aws("SSO OIDC returned no client secret".into()))?
        .to_owned();

    let auth = client
        .start_device_authorization()
        .client_id(&client_id)
        .client_secret(&client_secret)
        .start_url(&sso.start_url)
        .send()
        .await
        .map_err(Error::aws)?;
    let missing = |what: &str| Error::Aws(format!("SSO device authorization returned no {what}"));

    let prompt = SsoLoginPrompt {
        verification_uri: auth
            .verification_uri()
            .ok_or_else(|| missing("verification URI"))?
            .to_owned(),
        verification_uri_complete: auth.verification_uri_complete().map(str::to_owned),
        user_code: auth
            .user_code()
            .ok_or_else(|| missing("user code"))?
            .to_owned(),
        expires_in: auth.expires_in(),
        interval: auth.interval().max(1),
    };
    let pending = PendingLogin {
        sso: sso.clone(),
        client_id,
        client_secret,
        device_code: auth
            .device_code()
            .ok_or_else(|| missing("device code"))?
            .to_owned(),
        expires_at: unix_now() + auth.expires_in().max(0) as u64,
    };
    Ok((pending, prompt))
}

/// Polls once. On approval the token is saved to the keychain.
pub async fn poll(pending: &PendingLogin, secrets: &dyn SecretStore) -> Result<SsoLoginStatus> {
    if unix_now() >= pending.expires_at {
        return Ok(SsoLoginStatus::Expired);
    }
    let result = oidc_client(&pending.sso.region)
        .create_token()
        .client_id(&pending.client_id)
        .client_secret(&pending.client_secret)
        .grant_type(DEVICE_GRANT)
        .device_code(&pending.device_code)
        .send()
        .await;

    match result {
        Ok(out) => {
            let access_token = out
                .access_token()
                .ok_or_else(|| Error::Aws("SSO returned no access token".into()))?
                .to_owned();
            let token = SsoToken {
                access_token,
                expires_at: unix_now() + out.expires_in().max(0) as u64,
            };
            store_sso_token(secrets, &pending.sso, &token)?;
            Ok(SsoLoginStatus::Complete)
        }
        Err(err) => match err.as_service_error() {
            Some(e) if e.is_authorization_pending_exception() || e.is_slow_down_exception() => {
                Ok(SsoLoginStatus::Pending)
            }
            Some(e) if e.is_expired_token_exception() => Ok(SsoLoginStatus::Expired),
            _ => Err(Error::aws(err)),
        },
    }
}
