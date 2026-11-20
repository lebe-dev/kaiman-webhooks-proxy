use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::header;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::{Url, form_urlencoded};

mod config;
pub use config::{CALLBACK_PATH, OAuthConfig};

const MAX_FLOWS: usize = 1024;
const MAX_SESSIONS: usize = 4096;
pub const FLOW_TTL: Duration = Duration::from_secs(300);
const MAX_PROVIDER_RESPONSE_BYTES: usize = 256 * 1024;

struct PendingLogin {
    state: String,
    verifier: String,
    expires_at: Instant,
}

#[derive(Clone)]
pub struct Account {
    pub username: Option<String>,
}

struct Session {
    account: Account,
    expires_at: Instant,
}

#[derive(Default)]
struct LoginStore {
    pending: HashMap<String, PendingLogin>,
    sessions: HashMap<String, Session>,
}

pub struct OAuth {
    pub config: OAuthConfig,
    pub origin: String,
    client: reqwest::Client,
    store: Mutex<LoginStore>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
}

impl OAuth {
    pub fn new(config: OAuthConfig) -> anyhow::Result<Self> {
        crate::http_client::install_crypto_provider();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            origin: config.redirect_url.origin().ascii_serialization(),
            config,
            client,
            store: Mutex::default(),
        })
    }

    pub fn start(&self, previous_flow_id: Option<&str>) -> anyhow::Result<(String, Url)> {
        let flow_id = random_secret()?;
        let state = random_secret()?;
        let verifier = random_secret()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier));

        let mut redirect = self.config.authorize_url.clone();
        let mut query = redirect.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", self.config.redirect_url.as_str())
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        if !self.config.scopes.is_empty() {
            query.append_pair("scope", &self.config.scopes);
        }
        drop(query);

        let mut store = self.store.lock().expect("OAuth store lock");
        if let Some(previous) = previous_flow_id {
            store.pending.remove(previous);
        }
        let now = Instant::now();
        if store.pending.len() >= MAX_FLOWS {
            store.pending.retain(|_, login| login.expires_at > now);
        }
        anyhow::ensure!(
            store.pending.len() < MAX_FLOWS,
            "OAuth login capacity reached"
        );
        store.pending.insert(
            flow_id.clone(),
            PendingLogin {
                state,
                verifier,
                expires_at: now + FLOW_TTL,
            },
        );
        Ok((flow_id, redirect))
    }

    pub fn consume_login(&self, flow_id: &str, state: &str) -> Option<String> {
        let mut store = self.store.lock().expect("OAuth store lock");
        let login = store.pending.get(flow_id)?;
        if login.expires_at <= Instant::now()
            || !bool::from(login.state.as_bytes().ct_eq(state.as_bytes()))
        {
            return None;
        }
        store.pending.remove(flow_id).map(|login| login.verifier)
    }

    pub async fn authorize_account(
        &self,
        code: &str,
        verifier: &str,
    ) -> anyhow::Result<Option<Account>> {
        let mut params = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.config.redirect_url.as_str()),
            ("code_verifier", verifier),
        ];

        let mut request = self
            .client
            .post(self.config.token_url.clone())
            .header(header::ACCEPT, "application/json");
        match self.config.client_auth {
            config::ClientAuth::Post => {
                params.extend([
                    ("client_id", self.config.client_id.as_str()),
                    ("client_secret", self.config.client_secret.as_str()),
                ]);
            }
            config::ClientAuth::Basic => {
                let encode = |value: &str| {
                    form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
                };
                request = request.basic_auth(
                    encode(&self.config.client_id),
                    Some(encode(&self.config.client_secret)),
                );
            }
        }

        let token: TokenResponse = provider_json(request.form(&params))
            .await
            .context("OAuth token exchange failed")?;
        anyhow::ensure!(
            token.token_type.eq_ignore_ascii_case("bearer") && !token.access_token.is_empty(),
            "OAuth provider did not return a bearer token"
        );
        let profile: serde_json::Value = provider_json(
            self.client
                .get(self.config.userinfo_url.clone())
                .header(header::ACCEPT, "application/json")
                .bearer_auth(token.access_token),
        )
        .await
        .context("OAuth user profile request failed")?;

        let username = profile
            .get("preferred_username")
            .or_else(|| profile.get("username"))
            .and_then(serde_json::Value::as_str);
        Ok(self
            .profile_is_allowed(&profile, username)
            .then(|| Account {
                username: username
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned),
            }))
    }

    fn profile_is_allowed(&self, profile: &serde_json::Value, username: Option<&str>) -> bool {
        let id_allowed = match profile.pointer(&self.config.user_id_pointer) {
            Some(serde_json::Value::String(id)) => self.config.allowed_user_ids.contains(id),
            Some(serde_json::Value::Number(id)) if id.is_i64() || id.is_u64() => {
                self.config.allowed_user_ids.contains(&id.to_string())
            }
            _ => false,
        };
        id_allowed
            || username.is_some_and(|username| self.config.allowed_usernames.contains(username))
            || profile
                .get("groups")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|groups| {
                    groups
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|group| self.config.allowed_groups.contains(group))
                })
    }

    pub fn create_session(
        &self,
        previous_session_id: Option<&str>,
        account: Account,
    ) -> anyhow::Result<String> {
        let id = random_secret()?;
        let mut store = self.store.lock().expect("OAuth store lock");
        if let Some(previous) = previous_session_id {
            store.sessions.remove(previous);
        }
        let now = Instant::now();
        if store.sessions.len() >= MAX_SESSIONS {
            store.sessions.retain(|_, session| session.expires_at > now);
        }
        anyhow::ensure!(
            store.sessions.len() < MAX_SESSIONS,
            "OAuth session capacity reached"
        );
        store.sessions.insert(
            id.clone(),
            Session {
                account,
                expires_at: now + self.config.session_ttl,
            },
        );
        Ok(id)
    }

    pub fn session_account(&self, id: &str) -> Option<Account> {
        let store = self.store.lock().expect("OAuth store lock");
        store
            .sessions
            .get(id)
            .filter(|session| session.expires_at > Instant::now())
            .map(|session| session.account.clone())
    }

    pub fn logout(&self, id: &str) {
        self.store
            .lock()
            .expect("OAuth store lock")
            .sessions
            .remove(id);
    }
}

fn random_secret() -> anyhow::Result<String> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("OAuth random number generation failed"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

async fn provider_json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> anyhow::Result<T> {
    let mut response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("provider request failed"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "provider returned HTTP {}",
        response.status()
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("provider response could not be read"))?
    {
        anyhow::ensure!(
            body.len() + chunk.len() <= MAX_PROVIDER_RESPONSE_BYTES,
            "provider response is too large"
        );
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| anyhow::anyhow!("provider returned an invalid JSON response"))
}

#[cfg(test)]
mod tests;
