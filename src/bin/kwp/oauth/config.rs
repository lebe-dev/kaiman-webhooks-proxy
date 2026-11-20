use std::collections::HashSet;
use std::env;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use url::Url;

pub const CALLBACK_PATH: &str = "/api/auth/oauth/callback";

pub enum ClientAuth {
    Post,
    Basic,
}

pub struct OAuthConfig {
    pub provider_name: String,
    pub authorize_url: Url,
    pub token_url: Url,
    pub userinfo_url: Url,
    pub redirect_url: Url,
    pub client_id: String,
    pub client_secret: String,
    pub client_auth: ClientAuth,
    pub scopes: String,
    pub user_id_pointer: String,
    pub allowed_user_ids: HashSet<String>,
    pub session_ttl: Duration,
}

impl OAuthConfig {
    pub fn from_env() -> Result<Option<Self>> {
        Self::parse(|key| match env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(env::VarError::NotPresent) => Ok(None),
            Err(_) => anyhow::bail!("{key} must be valid UTF-8"),
        })
    }

    pub(super) fn parse(read: impl Fn(&str) -> Result<Option<String>>) -> Result<Option<Self>> {
        match read("AUTH_TYPE")?.as_deref() {
            None | Some("token") => return Ok(None),
            Some("oauth") => {}
            _ => anyhow::bail!("AUTH_TYPE must be token or oauth"),
        }

        let required = |key| {
            read(key)?
                .filter(|value| !value.trim().is_empty())
                .with_context(|| format!("{key} is required when OAuth is enabled"))
        };
        let endpoint = |key| {
            let url =
                Url::parse(&required(key)?).with_context(|| format!("{key} must be a URL"))?;
            let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            ensure!(
                url.scheme() == "https" || (url.scheme() == "http" && loopback),
                "{key} must use HTTPS (HTTP is allowed on loopback)"
            );
            ensure!(
                url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none(),
                "{key} must have a host and cannot contain credentials or a fragment"
            );
            Ok::<_, anyhow::Error>(url)
        };

        let redirect_url = endpoint("OAUTH_REDIRECT_URL")?;
        ensure!(
            redirect_url.path() == CALLBACK_PATH && redirect_url.query().is_none(),
            "OAUTH_REDIRECT_URL must end in {CALLBACK_PATH} and have no query"
        );
        let authorize_url = endpoint("OAUTH_AUTHORIZE_URL")?;
        ensure!(
            !authorize_url.query_pairs().any(|(key, _)| matches!(
                key.as_ref(),
                "response_type"
                    | "response_mode"
                    | "client_id"
                    | "redirect_uri"
                    | "scope"
                    | "state"
                    | "code_challenge"
                    | "code_challenge_method"
            )),
            "OAUTH_AUTHORIZE_URL cannot override OAuth request parameters"
        );

        let allowed_user_ids: HashSet<_> = required("OAUTH_ALLOWED_USER_IDS")?
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect();
        ensure!(
            !allowed_user_ids.is_empty(),
            "OAUTH_ALLOWED_USER_IDS must contain an account ID"
        );

        let user_id_pointer = read("OAUTH_USER_ID_POINTER")?.unwrap_or_else(|| "/sub".to_owned());
        ensure!(
            user_id_pointer.starts_with('/'),
            "OAUTH_USER_ID_POINTER must be a JSON Pointer starting with /"
        );

        let session_seconds = read("OAUTH_SESSION_TTL_SECONDS")?
            .map_or(Ok(3600), |value| value.parse::<u64>())
            .context("OAUTH_SESSION_TTL_SECONDS must be an integer")?;
        ensure!(
            (1..=86_400).contains(&session_seconds),
            "OAUTH_SESSION_TTL_SECONDS must be between 1 and 86400"
        );

        let client_auth = match read("OAUTH_TOKEN_AUTH_METHOD")?.as_deref() {
            None | Some("client_secret_post") => ClientAuth::Post,
            Some("client_secret_basic") => ClientAuth::Basic,
            _ => anyhow::bail!(
                "OAUTH_TOKEN_AUTH_METHOD must be client_secret_post or client_secret_basic"
            ),
        };

        Ok(Some(Self {
            provider_name: read("OAUTH_PROVIDER_NAME")?
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "OAuth".to_owned()),
            authorize_url,
            token_url: endpoint("OAUTH_TOKEN_URL")?,
            userinfo_url: endpoint("OAUTH_USERINFO_URL")?,
            redirect_url,
            client_id: required("OAUTH_CLIENT_ID")?,
            client_secret: required("OAUTH_CLIENT_SECRET")?,
            client_auth,
            scopes: read("OAUTH_SCOPES")?.unwrap_or_default(),
            user_id_pointer,
            allowed_user_ids,
            session_ttl: Duration::from_secs(session_seconds),
        }))
    }
}
