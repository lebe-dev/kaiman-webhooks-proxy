use std::sync::Arc;

use axum::{
    Json,
    extract::{FromRequestParts, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use kwp_lib::domain::config::model::WebhookChannelConfig;
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    middleware::request_id::RequestId,
    oauth::{FLOW_TTL, OAuth},
};

const FLOW_COOKIE: &str = "kwp_oauth_flow";
const SESSION_COOKIE: &str = "kwp_oauth_session";
type Rejection = (StatusCode, &'static str);

pub enum ApiAccess {
    Channel(usize),
    Operator,
}

impl FromRequestParts<Arc<AppState>> for ApiAccess {
    type Rejection = Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let authorization = parts.headers.get(header::AUTHORIZATION);
        let access = if let Some(oauth) = &state.oauth {
            if authorization.is_none() && has_session(oauth, &parts.headers) {
                if !parts.method.is_safe() && !same_origin(oauth, &parts.headers) {
                    log::warn!(
                        "rejected OAuth session request with invalid origin for {}",
                        parts.uri.path()
                    );
                    return Err((StatusCode::FORBIDDEN, "Forbidden"));
                }
                Some(Self::Operator)
            } else {
                None
            }
        } else {
            authorization
                .and_then(|header| header.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .and_then(|token| {
                    if let Some(index) = state.config.channel_index_by_token(token) {
                        Some(Self::Channel(index))
                    } else if state.config.is_ui_token(token) {
                        Some(Self::Operator)
                    } else {
                        None
                    }
                })
        };

        access.ok_or_else(|| {
            log::warn!(
                "[req:{}] unauthorized {} {}",
                parts
                    .extensions
                    .get::<RequestId>()
                    .map_or("-", RequestId::as_str),
                parts.method,
                parts.uri.path()
            );
            (StatusCode::UNAUTHORIZED, "Unauthorized")
        })
    }
}

impl ApiAccess {
    pub fn channel<'a>(
        &self,
        state: &'a AppState,
        request_id: &RequestId,
        channel_name: &str,
    ) -> Result<&'a WebhookChannelConfig, Rejection> {
        if let Self::Channel(index) = self {
            let channel = &state.config.channels[*index];
            if channel.name != channel_name {
                log::warn!(
                    "[req:{request_id}] token for channel '{}' used to access channel '{channel_name}'",
                    channel.name
                );
                return Err((StatusCode::FORBIDDEN, "Forbidden"));
            }
            return Ok(channel);
        }
        state
            .config
            .find_channel_by_name(channel_name)
            .ok_or((StatusCode::NOT_FOUND, "Channel not found"))
    }
}

#[derive(Serialize)]
#[serde(tag = "authType", rename_all = "lowercase")]
enum AuthStatus<'a> {
    Token,
    OAuth {
        provider: &'a str,
        authenticated: bool,
    },
}

pub async fn auth_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let status = match &state.oauth {
        Some(oauth) => AuthStatus::OAuth {
            provider: &oauth.config.provider_name,
            authenticated: has_session(oauth, &headers),
        },
        None => AuthStatus::Token,
    };
    Json(status).into_response()
}

pub async fn oauth_start(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(oauth) = &state.oauth else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match oauth.start(get_cookie(oauth, &headers, FLOW_COOKIE)) {
        Ok((flow_id, redirect)) => {
            let mut response = Redirect::to(redirect.as_str()).into_response();
            set_cookie(
                &mut response,
                oauth,
                FLOW_COOKIE,
                &flow_id,
                FLOW_TTL.as_secs(),
            );
            response
        }
        Err(error) => {
            log::warn!("OAuth login could not start: {error}");
            login_error("unavailable")
        }
    }
}

#[derive(Deserialize)]
pub struct OAuthCallback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

pub async fn oauth_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    callback: Result<Query<OAuthCallback>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(oauth) = &state.oauth else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(Query(callback)) = callback else {
        return login_error("invalid");
    };
    let Some((flow_id, received_state)) =
        get_cookie(oauth, &headers, FLOW_COOKIE).zip(callback.state.as_deref())
    else {
        return login_error("invalid");
    };
    let Some(verifier) = oauth.consume_login(flow_id, received_state) else {
        return login_error("invalid");
    };

    let mut response = complete_login(oauth, &headers, callback, &verifier).await;
    set_cookie(&mut response, oauth, FLOW_COOKIE, "", 0);
    response
}

async fn complete_login(
    oauth: &OAuth,
    headers: &HeaderMap,
    callback: OAuthCallback,
    verifier: &str,
) -> Response {
    if callback.error.is_some() {
        return login_error("denied");
    }
    let Some(code) = callback.code.filter(|code| !code.is_empty()) else {
        return login_error("invalid");
    };
    match oauth.authorize_account(&code, verifier).await {
        Ok(true) => {}
        Ok(false) => return login_error("denied"),
        Err(error) => {
            log::warn!("OAuth login failed: {error:#}");
            return login_error("unavailable");
        }
    }
    let session_id = match oauth.create_session(get_cookie(oauth, headers, SESSION_COOKIE)) {
        Ok(id) => id,
        Err(error) => {
            log::warn!("OAuth session could not be created: {error}");
            return login_error("unavailable");
        }
    };
    let mut response = Redirect::to("/").into_response();
    set_cookie(
        &mut response,
        oauth,
        SESSION_COOKIE,
        &session_id,
        oauth.config.session_ttl.as_secs(),
    );
    response
}

pub async fn oauth_logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(oauth) = &state.oauth else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !same_origin(oauth, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(id) = get_cookie(oauth, &headers, SESSION_COOKIE) {
        oauth.logout(id);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    set_cookie(&mut response, oauth, SESSION_COOKIE, "", 0);
    response
}

pub async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn no_referrer(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn login_error(reason: &str) -> Response {
    Redirect::to(&format!("/?oauth_error={reason}")).into_response()
}

fn same_origin(oauth: &OAuth, headers: &HeaderMap) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    origins.next().is_some_and(|origin| origin == &oauth.origin) && origins.next().is_none()
}

fn has_session(oauth: &OAuth, headers: &HeaderMap) -> bool {
    get_cookie(oauth, headers, SESSION_COOKIE).is_some_and(|id| oauth.has_session(id))
}

fn cookie_name(oauth: &OAuth, name: &str) -> String {
    if oauth.config.redirect_url.scheme() == "https" {
        format!("__Host-{name}")
    } else {
        name.to_owned()
    }
}

fn get_cookie<'a>(oauth: &OAuth, headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let name = cookie_name(oauth, name);
    let mut cookies = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .filter_map(|(key, value)| (key == name).then_some(value));
    let value = cookies.next()?;
    cookies.next().is_none().then_some(value)
}

fn set_cookie(response: &mut Response, oauth: &OAuth, name: &str, value: &str, max_age: u64) {
    let name = cookie_name(oauth, name);
    let secure = if oauth.config.redirect_url.scheme() == "https" {
        "; Secure"
    } else {
        ""
    };
    let cookie =
        format!("{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}");
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("valid OAuth cookie"),
    );
}

pub fn strip_oauth_cookies(header: &str) -> Option<String> {
    let is_oauth_cookie = |cookie: &str| {
        cookie.split_once('=').is_some_and(|(name, _)| {
            matches!(
                name.trim().strip_prefix("__Host-").unwrap_or(name.trim()),
                FLOW_COOKIE | SESSION_COOKIE
            )
        })
    };
    if !header.split(';').any(is_oauth_cookie) {
        return Some(header.to_owned());
    }
    let sanitized = header
        .split(';')
        .map(str::trim)
        .filter(|cookie| !cookie.is_empty() && !is_oauth_cookie(cookie))
        .collect::<Vec<_>>()
        .join("; ");
    (!sanitized.is_empty()).then_some(sanitized)
}
