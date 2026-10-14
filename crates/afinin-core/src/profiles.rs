// SPDX-License-Identifier: MIT OR Apache-2.0

//! Discovery of AWS CLI profiles from `~/.aws/config` and `~/.aws/credentials`
//! (FR-1). Only names and non-secret settings are read; secret values are never
//! retained.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// IAM Identity Center — recommended.
    Sso,
    /// `role_arn` + `source_profile` / `credential_source` chain.
    AssumeRole,
    /// `aws login` console session (`login_session`) — short-lived, like SSO.
    Login,
    /// `credential_process` helper.
    CredentialProcess,
    /// `web_identity_token_file`.
    WebIdentity,
    /// Long-lived IAM access keys — discouraged.
    StaticKeys,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SsoSettings {
    /// `sso_session` name, when the profile uses the newer session format.
    pub session_name: Option<String>,
    pub start_url: String,
    pub region: String,
    pub account_id: Option<String>,
    pub role_name: Option<String>,
}

impl SsoSettings {
    /// Key identifying the cached SSO token: the session name if present,
    /// otherwise the start URL (same convention as the AWS CLI).
    pub fn token_key(&self) -> &str {
        self.session_name.as_deref().unwrap_or(&self.start_url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AwsProfile {
    pub name: String,
    pub kind: ProfileKind,
    pub region: Option<String>,
    pub sso: Option<SsoSettings>,
    pub role_arn: Option<String>,
    pub source_profile: Option<String>,
}

impl AwsProfile {
    /// Long-lived keys are supported but discouraged (FR-1.4).
    pub fn is_discouraged(&self) -> bool {
        self.kind == ProfileKind::StaticKeys
    }
}

type Section = BTreeMap<String, String>;

#[derive(Debug, Default)]
struct IniFile {
    sections: BTreeMap<String, Section>,
}

fn parse_ini(text: &str) -> IniFile {
    let mut file = IniFile::default();
    let mut current: Option<String> = None;
    for raw in text.lines() {
        // Indented lines are sub-properties (e.g. `s3 =\n  max_concurrent_requests = 10`).
        if raw.starts_with([' ', '\t']) {
            continue;
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = header.split_whitespace().collect::<Vec<_>>().join(" ");
            file.sections.entry(name.clone()).or_default();
            current = Some(name);
            continue;
        }
        if let (Some(section), Some((k, v))) = (&current, line.split_once('=')) {
            file.sections
                .get_mut(section)
                .expect("section inserted on header")
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    file
}

/// Locations of the shared AWS config files, honoring the standard env vars.
#[derive(Debug, Clone)]
pub struct AwsFiles {
    pub config: PathBuf,
    pub credentials: PathBuf,
}

impl AwsFiles {
    pub fn from_env() -> Self {
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .unwrap_or_default();
        let config = std::env::var_os("AWS_CONFIG_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".aws").join("config"));
        let credentials = std::env::var_os("AWS_SHARED_CREDENTIALS_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".aws").join("credentials"));
        Self {
            config,
            credentials,
        }
    }
}

fn read_optional(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

/// Lists all profiles defined in the shared config and credentials files.
pub fn list_profiles(files: &AwsFiles) -> Result<Vec<AwsProfile>> {
    let config = read_optional(&files.config)?;
    let credentials = read_optional(&files.credentials)?;
    parse_profiles(&config, &credentials)
}

pub fn find_profile(files: &AwsFiles, name: &str) -> Result<AwsProfile> {
    list_profiles(files)?
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| Error::ProfileNotFound(name.to_owned()))
}

pub fn parse_profiles(config: &str, credentials: &str) -> Result<Vec<AwsProfile>> {
    let config = parse_ini(config);
    let credentials = parse_ini(credentials);

    let mut sessions: BTreeMap<&str, &Section> = BTreeMap::new();
    let mut merged: BTreeMap<String, Section> = BTreeMap::new();

    for (header, section) in &config.sections {
        if let Some(name) = header.strip_prefix("sso-session ") {
            sessions.insert(name, section);
        } else if let Some(name) = header.strip_prefix("profile ") {
            merged
                .entry(name.to_owned())
                .or_default()
                .extend(section.clone());
        } else if header == "default" {
            merged
                .entry("default".into())
                .or_default()
                .extend(section.clone());
        }
    }
    // Credentials file wins over config for the same keys, as in the SDK.
    for (name, section) in &credentials.sections {
        merged
            .entry(name.clone())
            .or_default()
            .extend(section.clone());
    }

    merged
        .into_iter()
        .map(|(name, s)| build_profile(name, &s, &sessions))
        .collect()
}

fn build_profile(
    name: String,
    s: &Section,
    sessions: &BTreeMap<&str, &Section>,
) -> Result<AwsProfile> {
    let get = |k: &str| s.get(k).cloned();

    let sso = if let Some(session_name) = get("sso_session") {
        let session = sessions.get(session_name.as_str()).ok_or_else(|| {
            Error::InvalidProfile(format!(
                "profile `{name}` references missing sso-session `{session_name}`"
            ))
        })?;
        let field = |k: &str| {
            session.get(k).cloned().ok_or_else(|| {
                Error::InvalidProfile(format!("sso-session `{session_name}` is missing `{k}`"))
            })
        };
        Some(SsoSettings {
            start_url: field("sso_start_url")?,
            region: field("sso_region")?,
            session_name: Some(session_name),
            account_id: get("sso_account_id"),
            role_name: get("sso_role_name"),
        })
    } else if let Some(start_url) = get("sso_start_url") {
        Some(SsoSettings {
            session_name: None,
            region: get("sso_region").ok_or_else(|| {
                Error::InvalidProfile(format!("profile `{name}` is missing `sso_region`"))
            })?,
            start_url,
            account_id: get("sso_account_id"),
            role_name: get("sso_role_name"),
        })
    } else {
        None
    };

    // Same precedence as the AWS SDK credential chain.
    let kind = if s.contains_key("role_arn") && !s.contains_key("web_identity_token_file") {
        ProfileKind::AssumeRole
    } else if s.contains_key("web_identity_token_file") {
        ProfileKind::WebIdentity
    } else if sso.is_some() {
        ProfileKind::Sso
    } else if s.contains_key("login_session") {
        ProfileKind::Login
    } else if s.contains_key("credential_process") {
        ProfileKind::CredentialProcess
    } else if s.contains_key("aws_access_key_id") {
        ProfileKind::StaticKeys
    } else {
        ProfileKind::Unknown
    };

    Ok(AwsProfile {
        region: get("region"),
        role_arn: get("role_arn"),
        source_profile: get("source_profile"),
        sso,
        kind,
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
[default]
region = us-east-1

[profile legacy-sso]
sso_start_url = https://legacy.awsapps.com/start
sso_region = us-east-1
sso_account_id = 111111111111
sso_role_name = ReadOnly

[sso-session corp]
sso_start_url = https://corp.awsapps.com/start
sso_region = eu-west-1
sso_registration_scopes = sso:account:access

[profile prod]
sso_session = corp
sso_account_id = 222222222222
sso_role_name = ViewOnly
region = eu-west-1
s3 =
  max_concurrent_requests = 10

[profile audit]
role_arn = arn:aws:iam::333333333333:role/AfininReadOnly
source_profile = prod

[profile console]
login_session = arn:aws:iam::444444444444:user/admin
region = us-east-1
"#;

    const CREDENTIALS: &str = r#"
[default]
aws_access_key_id = AKIAEXAMPLE
aws_secret_access_key = secret
"#;

    fn by_name<'a>(ps: &'a [AwsProfile], n: &str) -> &'a AwsProfile {
        ps.iter().find(|p| p.name == n).unwrap()
    }

    #[test]
    fn parses_all_profile_kinds() {
        let ps = parse_profiles(CONFIG, CREDENTIALS).unwrap();
        assert_eq!(ps.len(), 5);

        let default = by_name(&ps, "default");
        assert_eq!(default.kind, ProfileKind::StaticKeys);
        assert!(default.is_discouraged());
        assert_eq!(default.region.as_deref(), Some("us-east-1"));

        let legacy = by_name(&ps, "legacy-sso");
        assert_eq!(legacy.kind, ProfileKind::Sso);
        let sso = legacy.sso.as_ref().unwrap();
        assert_eq!(sso.token_key(), "https://legacy.awsapps.com/start");

        let prod = by_name(&ps, "prod");
        assert_eq!(prod.kind, ProfileKind::Sso);
        let sso = prod.sso.as_ref().unwrap();
        assert_eq!(sso.region, "eu-west-1");
        assert_eq!(sso.token_key(), "corp");
        assert_eq!(sso.account_id.as_deref(), Some("222222222222"));

        let audit = by_name(&ps, "audit");
        assert_eq!(audit.kind, ProfileKind::AssumeRole);
        assert_eq!(audit.source_profile.as_deref(), Some("prod"));

        assert_eq!(by_name(&ps, "console").kind, ProfileKind::Login);
    }

    #[test]
    fn missing_session_is_an_error() {
        let cfg = "[profile x]\nsso_session = nope\n";
        assert!(matches!(
            parse_profiles(cfg, ""),
            Err(Error::InvalidProfile(_))
        ));
    }

    #[test]
    fn serialized_profiles_never_contain_secrets() {
        let ps = parse_profiles(CONFIG, CREDENTIALS).unwrap();
        let json = serde_json::to_string(&ps).unwrap();
        assert!(!json.contains("AKIAEXAMPLE"));
        assert!(!json.contains("secret"));
    }
}
