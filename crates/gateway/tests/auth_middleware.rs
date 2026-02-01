//! Integration tests for the auth middleware protecting API endpoints.

use std::{net::SocketAddr, sync::Arc};

use tokio::net::TcpListener;

use moltis_gateway::{
    auth::{self, CredentialStore},
    methods::MethodRegistry,
    server::build_gateway_app,
    services::GatewayServices,
    state::GatewayState,
};

/// Start a test server with a credential store (auth enabled).
async fn start_auth_server() -> (SocketAddr, Arc<CredentialStore>) {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    let cred_store = Arc::new(CredentialStore::new(pool).await.unwrap());

    let resolved_auth = auth::resolve_auth(None, None);
    let services = GatewayServices::noop();
    let state = GatewayState::with_options(
        resolved_auth,
        services,
        Arc::new(moltis_tools::approval::ApprovalManager::default()),
        None,
        Some(Arc::clone(&cred_store)),
        None,
    );
    let methods = Arc::new(MethodRegistry::new());
    let app = build_gateway_app(state, methods);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, cred_store)
}

/// Start a test server without a credential store (no auth).
async fn start_noauth_server() -> SocketAddr {
    let resolved_auth = auth::resolve_auth(None, None);
    let services = GatewayServices::noop();
    let state = GatewayState::new(
        resolved_auth,
        services,
        Arc::new(moltis_tools::approval::ApprovalManager::default()),
    );
    let methods = Arc::new(MethodRegistry::new());
    let app = build_gateway_app(state, methods);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

/// When no credential store is configured, all API routes pass through.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn no_auth_configured_passes_through() {
    let addr = start_noauth_server().await;
    let resp = reqwest::get(format!("http://{addr}/api/bootstrap"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// When auth is configured but setup is not complete (no password set),
/// all API routes pass through.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn setup_not_complete_passes_through() {
    let (addr, _store) = start_auth_server().await;
    // No password set yet, so setup is not complete.
    let resp = reqwest::get(format!("http://{addr}/api/bootstrap"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// When auth is configured and setup is complete, unauthenticated requests
/// to protected endpoints return 401.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn unauthenticated_returns_401() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();

    let resp = reqwest::get(format!("http://{addr}/api/bootstrap"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "not authenticated");
}

/// Authenticated request with a valid session cookie succeeds.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn session_cookie_auth_succeeds() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();
    let token = store.create_session().await.unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/bootstrap"))
        .header("Cookie", format!("moltis_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// Authenticated request with a valid API key in Bearer header succeeds.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn api_key_auth_succeeds() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();
    let (_id, raw_key) = store.create_api_key("test").await.unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/bootstrap"))
        .header("Authorization", format!("Bearer {raw_key}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// Unauthenticated request to /api/images/cached returns 401 when auth is set up.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn images_endpoint_returns_401() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();

    let resp = reqwest::get(format!("http://{addr}/api/images/cached"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// Public routes remain accessible without auth even when auth is configured.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn public_routes_accessible_without_auth() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();

    // /health is always public.
    let resp = reqwest::get(format!("http://{addr}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // /api/auth/status is public.
    let resp = reqwest::get(format!("http://{addr}/api/auth/status"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // SPA fallback (root page) is public.
    let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
    assert_eq!(resp.status(), 200);
}

/// Invalid session cookie returns 401.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn invalid_session_cookie_returns_401() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/bootstrap"))
        .header("Cookie", "moltis_session=invalid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// POST /api/auth/reset removes all auth and subsequent requests pass through.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn reset_auth_removes_all_authentication() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();
    let token = store.create_session().await.unwrap();

    // Protected endpoint requires auth.
    let resp = reqwest::get(format!("http://{addr}/api/bootstrap"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Reset auth (requires session).
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/auth/reset"))
        .header("Cookie", format!("moltis_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Now auth is disabled, so middleware passes through.
    let resp = reqwest::get(format!("http://{addr}/api/bootstrap"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // /api/auth/status should report authenticated: true (no login needed).
    let resp = reqwest::get(format!("http://{addr}/api/auth/status"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["setup_required"], false);
}

/// Reset without session returns 401.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn reset_auth_requires_session() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/api/auth/reset"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// Revoked API key returns 401.
#[cfg(feature = "web-ui")]
#[tokio::test]
async fn revoked_api_key_returns_401() {
    let (addr, store) = start_auth_server().await;
    store.set_initial_password("testpass123").await.unwrap();
    let (id, raw_key) = store.create_api_key("test").await.unwrap();
    store.revoke_api_key(id).await.unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/bootstrap"))
        .header("Authorization", format!("Bearer {raw_key}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
