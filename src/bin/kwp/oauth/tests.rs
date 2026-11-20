use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Extension, Form, Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use kwp_lib::domain::config::model::{AppConfig, WebhookChannelConfig};
use kwp_lib::domain::webhook::service::WebhookServiceImpl;
use kwp_lib::outbound::sqlite::Sqlite;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::{AppState, build_router, middleware};

#[derive(Default)]
struct Provider {
    exchanges: AtomicUsize,
    challenge: Mutex<String>,
}

async fn exchange(
    State(provider): State<Arc<Provider>>,
    Form(params): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(params["client_id"], "kwp");
    assert_eq!(params["client_secret"], "client-secret");
    assert_eq!(params["grant_type"], "authorization_code");
    assert_eq!(
        params["redirect_uri"],
        format!("https://kwp.example.com{CALLBACK_PATH}")
    );
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(&params["code_verifier"])),
        *provider.challenge.lock().unwrap()
    );
    provider.exchanges.fetch_add(1, Ordering::SeqCst);
    Json(json!({
        "access_token": params["code"],
        "token_type": "Bearer",
        "refresh_token": "unused"
    }))
}

async fn userinfo(headers: HeaderMap) -> Json<Value> {
    Json(
        match headers
            .get(header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap()
        {
            "Bearer allowed" => json!({
                "id": 42,
                "preferred_username": "operator",
                "username": "legacy"
            }),
            "Bearer username" => json!({ "sub": "100", "preferred_username": "alice" }),
            "Bearer gitlab-api" => json!({ "id": 100, "username": "alice" }),
            "Bearer group" => json!({ "sub": "100", "groups": ["other", "engineering/on-call"] }),
            "Bearer denied" => json!({
                "id": 100,
                "preferred_username": "alice2",
                "groups": ["other/on-call", "engineering/on-call/nested"]
            }),
            "Bearer malformed" => json!({ "groups": "engineering/on-call" }),
            _ => panic!("unexpected token"),
        },
    )
}

fn settings(key: &str, provider: &str) -> Option<String> {
    Some(match key {
        "AUTH_TYPE" => "oauth".into(),
        "OAUTH_PROVIDER_NAME" => "Example".into(),
        "OAUTH_AUTHORIZE_URL" => format!("{provider}/authorize"),
        "OAUTH_TOKEN_URL" => format!("{provider}/token"),
        "OAUTH_USERINFO_URL" => format!("{provider}/userinfo"),
        "OAUTH_REDIRECT_URL" => format!("https://kwp.example.com{CALLBACK_PATH}"),
        "OAUTH_CLIENT_ID" => "kwp".into(),
        "OAUTH_CLIENT_SECRET" => "client-secret".into(),
        "OAUTH_SCOPES" => "openid".into(),
        "OAUTH_ALLOWED_USER_IDS" => "42, 9001".into(),
        "OAUTH_ALLOWED_USERNAMES" => "bob, alice".into(),
        "OAUTH_ALLOWED_GROUPS" => "platform, engineering/on-call".into(),
        "OAUTH_USER_ID_POINTER" => "/id".into(),
        _ => return None,
    })
}

async fn app(oauth: Option<OAuth>) -> Router {
    let channel = |name: &str| {
        serde_yaml::from_str::<WebhookChannelConfig>(&format!(
            "name: {name}\napi-read-token: {name}-token"
        ))
        .unwrap()
    };
    let config = AppConfig {
        bind: "127.0.0.1:8080".into(),
        log_level: "info".into(),
        log_target: "stdout".into(),
        data_path: ".".into(),
        db_cnn: "sqlite::memory:".into(),
        channels: vec![
            channel("a"),
            channel("b"),
            serde_yaml::from_str("name: omitted").unwrap(),
            serde_yaml::from_str("name: empty\napi-read-token: ''").unwrap(),
            serde_yaml::from_str("name: blank\napi-read-token: ' '").unwrap(),
        ],
        default_body_limit: 262_144,
        ignored_headers: vec![],
        metrics_enabled: false,
        trusted_proxies: vec![],
        ui_access_token: oauth.is_none().then(|| "ui-token".to_owned()),
        ui_enabled: true,
        api_enabled: true,
        forward_backoff: Default::default(),
    };
    let router = build_router(&config);
    let db = Sqlite::new(&config.db_cnn).await.unwrap();
    let state = Arc::new(AppState {
        config,
        oauth,
        webhook_service: WebhookServiceImpl::new(db),
        metrics_handle: None,
        http_client: crate::http_client::build_http_client().unwrap(),
        forward_statuses: Default::default(),
    });
    router
        .layer(Extension(middleware::client_ip::ClientIp(
            "127.0.0.1".parse().unwrap(),
        )))
        .layer(axum::middleware::from_fn(
            middleware::request_id::middleware,
        ))
        .with_state(state)
}

async fn send(app: &Router, method: Method, path: &str, headers: &[(&str, &str)]) -> Response {
    let mut req = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    app.clone()
        .oneshot(req.body(Body::from("{}")).unwrap())
        .await
        .unwrap()
}

async fn auth_status(app: &Router, cookie: &str) -> Value {
    let response = send(app, Method::GET, "/api/auth/status", &[("cookie", cookie)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

async fn start(app: &Router, provider: &Provider) -> (String, String) {
    let response = send(app, Method::GET, "/api/auth/oauth/start", &[]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("__Host-kwp_oauth_flow="));
    assert!(cookie.contains("Path=/; Max-Age=300; HttpOnly; SameSite=Lax; Secure"));
    let url = Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let query: HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["scope"], "openid");
    *provider.challenge.lock().unwrap() = query["code_challenge"].to_string();
    (
        query["state"].to_string(),
        cookie.split(';').next().unwrap().to_owned(),
    )
}

#[tokio::test]
async fn browser_session_through_real_routes() {
    let provider = Arc::new(Provider::default());
    let provider_app = Router::new()
        .route("/token", post(exchange))
        .route("/userinfo", get(userinfo))
        .with_state(provider.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async { axum::serve(listener, provider_app).await.unwrap() });
    let config = OAuthConfig::parse(|key| Ok(settings(key, &provider_url)))
        .unwrap()
        .unwrap();
    let app = app(Some(OAuth::new(config).unwrap())).await;

    let (state, flow) = start(&app, &provider).await;
    let callback = format!("{CALLBACK_PATH}?code=allowed&state={state}");
    for (path, cookies) in [
        (callback.as_str(), ""),
        (
            "/api/auth/oauth/callback?code=allowed&state=wrong",
            flow.as_str(),
        ),
    ] {
        let response = send(&app, Method::GET, path, &[("cookie", cookies)]).await;
        assert_eq!(
            response.headers()[header::LOCATION],
            "/?oauth_error=invalid"
        );
    }
    assert_eq!(provider.exchanges.load(Ordering::SeqCst), 0);

    let response = send(&app, Method::GET, &callback, &[("cookie", &flow)]).await;
    assert_eq!(response.headers()[header::LOCATION], "/");
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("__Host-kwp_oauth_session="));
    assert!(cookie.contains("Path=/; Max-Age=3600; HttpOnly; SameSite=Lax; Secure"));
    let session = cookie.split(';').next().unwrap().to_owned();

    let response = send(&app, Method::GET, &callback, &[("cookie", &flow)]).await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "/?oauth_error=invalid"
    );
    assert_eq!(provider.exchanges.load(Ordering::SeqCst), 1);

    assert_eq!(
        auth_status(&app, &session).await,
        json!({
            "authType": "oauth",
            "provider": "Example",
            "authenticated": true,
            "username": "operator"
        })
    );
    assert_eq!(
        send(&app, Method::GET, "/api/config", &[("cookie", &session)])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&app, Method::GET, "/api/webhook/b", &[("cookie", &session)])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/webhook/b/list",
            &[("cookie", &session), ("authorization", "Bearer a-token")]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );

    let incoming_cookie = format!("{session}; external=1; {flow}");
    assert!(
        send(
            &app,
            Method::POST,
            "/api/webhook/b",
            &[
                ("cookie", &incoming_cookie),
                ("content-type", "application/json")
            ]
        )
        .await
        .status()
        .is_success()
    );
    let list = send(
        &app,
        Method::GET,
        "/api/webhook/b/list",
        &[("cookie", &session)],
    )
    .await;
    assert_eq!(list.headers()[header::CACHE_CONTROL], "no-store");
    let webhooks: Value =
        serde_json::from_slice(&to_bytes(list.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(webhooks[0]["headers"]["cookie"], "external=1");
    let delete_path = format!("/api/webhook/b/{}", webhooks[0]["id"]);
    assert_eq!(
        send(
            &app,
            Method::DELETE,
            &delete_path,
            &[
                ("cookie", &session),
                ("origin", "https://other.example.com")
            ]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            Method::DELETE,
            &delete_path,
            &[("cookie", &session), ("origin", "https://kwp.example.com")]
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &app,
            Method::POST,
            "/api/auth/logout",
            &[("cookie", &session)]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            Method::POST,
            "/api/auth/logout",
            &[("cookie", &session), ("origin", "https://kwp.example.com")]
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        auth_status(&app, &session).await,
        json!({ "authType": "oauth", "provider": "Example", "authenticated": false })
    );
    assert_eq!(
        send(&app, Method::GET, "/api/config", &[("cookie", &session)])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/config",
            &[("authorization", "Bearer a-token")]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/webhook/a",
            &[("authorization", "Bearer a-token")]
        )
        .await
        .status(),
        StatusCode::OK
    );

    for (code, location, cookies, username) in [
        ("username", "/", 2, Some("alice")),
        ("gitlab-api", "/", 2, Some("alice")),
        ("group", "/", 2, None),
        ("denied", "/?oauth_error=denied", 1, None),
        ("malformed", "/?oauth_error=denied", 1, None),
    ] {
        let (state, flow) = start(&app, &provider).await;
        let response = send(
            &app,
            Method::GET,
            &format!("{CALLBACK_PATH}?code={code}&state={state}"),
            &[("cookie", &flow)],
        )
        .await;
        assert_eq!(response.headers()[header::LOCATION], location, "{code}");
        assert_eq!(
            response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .count(),
            cookies,
            "{code}"
        );
        if location == "/" {
            let session = response.headers()[header::SET_COOKIE].to_str().unwrap();
            let session = session.split(';').next().unwrap();
            let mut expected = json!({
                "authType": "oauth",
                "provider": "Example",
                "authenticated": true
            });
            if let Some(username) = username {
                expected["username"] = json!(username);
            }
            assert_eq!(auth_status(&app, session).await, expected, "{code}");
        }
    }
    server.abort();
}

#[test]
fn configuration_is_opt_in_and_requires_an_allowlist_and_safe_endpoints() {
    assert!(OAuthConfig::parse(|_| Ok(None)).unwrap().is_none());
    assert!(
        OAuthConfig::parse(|key| Ok((key == "AUTH_TYPE").then(|| "token".to_owned())))
            .unwrap()
            .is_none()
    );
    let all_allowlists = [
        "OAUTH_ALLOWED_USER_IDS",
        "OAUTH_ALLOWED_USERNAMES",
        "OAUTH_ALLOWED_GROUPS",
    ];
    for enabled in all_allowlists {
        let config = OAuthConfig::parse(|key| {
            Ok(if key != enabled && all_allowlists.contains(&key) {
                None
            } else {
                settings(key, "https://provider.example.com")
            })
        });
        assert!(config.unwrap().is_some(), "{enabled}");
    }
    let missing = OAuthConfig::parse(|key| {
        Ok(if all_allowlists.contains(&key) {
            Some(", ,".to_owned())
        } else {
            settings(key, "https://provider.example.com")
        })
    });
    let error = missing.err().unwrap().to_string();
    assert!(error.contains("must contain an entry"));
    for (variable, value) in [
        ("AUTH_TYPE", "typo"),
        ("OAUTH_TOKEN_URL", "http://provider.example.com/token"),
        (
            "OAUTH_AUTHORIZE_URL",
            "https://provider.example.com?state=override",
        ),
        ("OAUTH_REDIRECT_URL", "https://kwp.example.com/unrelated"),
        ("OAUTH_SESSION_TTL_SECONDS", "0"),
    ] {
        let result = OAuthConfig::parse(|key| {
            Ok(if key == variable {
                Some(value.to_owned())
            } else {
                settings(key, "https://provider.example.com")
            })
        });
        assert!(result.err().unwrap().to_string().contains(variable));
    }
}

#[tokio::test]
async fn token_mode_accepts_bearer_and_disables_oauth_routes() {
    let app = app(None).await;
    let response = send(&app, Method::GET, "/api/auth/status", &[]).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 4096).await.unwrap())
            .unwrap(),
        json!({ "authType": "token" })
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/config",
            &[("authorization", "Bearer ui-token")]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &app,
            Method::GET,
            "/api/config",
            &[("cookie", "__Host-kwp_oauth_session=unused")]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, Method::GET, "/api/auth/oauth/start", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn channels_without_usable_read_tokens_require_operator_access() {
    let app = app(None).await;
    for channel in ["omitted", "empty", "blank"] {
        let path = format!("/api/webhook/{channel}");
        assert_eq!(
            send(
                &app,
                Method::POST,
                &path,
                &[("content-type", "application/json")]
            )
            .await
            .status(),
            StatusCode::OK
        );

        for path in [&format!("{path}/list"), &path] {
            for (headers, expected) in [
                (&[][..], StatusCode::UNAUTHORIZED),
                (
                    &[("authorization", "Bearer ")][..],
                    StatusCode::UNAUTHORIZED,
                ),
                (
                    &[("authorization", "Bearer  ")][..],
                    StatusCode::UNAUTHORIZED,
                ),
                (
                    &[("authorization", "Bearer a-token")][..],
                    StatusCode::FORBIDDEN,
                ),
                (&[("authorization", "Bearer ui-token")][..], StatusCode::OK),
            ] {
                let response = send(&app, Method::GET, path, headers).await;
                assert_eq!(response.status(), expected, "{path} with {headers:?}");
            }
        }
    }

    for path in ["/api/webhook/a", "/api/webhook/a/list"] {
        assert_eq!(
            send(
                &app,
                Method::GET,
                path,
                &[("authorization", "Bearer a-token")]
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
}
