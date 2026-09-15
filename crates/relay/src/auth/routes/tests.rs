use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;

use crate::auth::oidc::{OidcError, OidcHttp};
use crate::auth::session::random_token;
use crate::config::Config;
use crate::routes::router;
use crate::slack::api::FakeSlack;
use crate::state::AppState;
use crate::store::{Store, TokenRecord};

const NOW: i64 = 1789560000;
const ADMIN: &str = "U0123ABCD";
const REDIRECT_URI: &str = "https://sccr.example.jp/auth/callback";

fn fixed_now() -> i64 {
    NOW
}

type TokenCall = (String, String, String, String);

#[derive(Default)]
struct FakeOidc {
    id_token: StdMutex<Option<String>>,
    calls: StdMutex<Vec<TokenCall>>,
}

impl FakeOidc {
    fn respond(&self, jwt: String) {
        *self.id_token.lock().unwrap() = Some(jwt);
    }
}

#[async_trait]
impl OidcHttp for FakeOidc {
    async fn token(
        &self,
        code: &str,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
    ) -> Result<String, OidcError> {
        self.calls.lock().unwrap().push((
            code.into(),
            client_id.into(),
            client_secret.into(),
            redirect_uri.into(),
        ));
        self.id_token
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| OidcError::Http("invalid_code".into()))
    }
}

fn jwt(sub: &str, nonce: &str) -> String {
    let payload = serde_json::json!({
        "iss": "https://slack.com",
        "sub": sub,
        "aud": "cid",
        "exp": NOW + 300,
        "nonce": nonce,
    });
    format!(
        "{}.{}.sig",
        URL_SAFE_NO_PAD.encode(b"{}"),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

struct Setup {
    state: Arc<AppState>,
    oidc: Arc<FakeOidc>,
}

fn setup_with(store: Store) -> Setup {
    let dir = std::env::temp_dir().join(format!("sccr-auth-{}", random_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = Config {
        bind: "127.0.0.1:0".into(),
        public_url: "https://sccr.example.jp".into(),
        state_file: dir.join("state.json"),
        admin_user: ADMIN.into(),
        signing_secret: "secret".into(),
        bot_token: "xoxb-test".into(),
        client_id: "cid".into(),
        client_secret: "csecret".into(),
        dev: false,
    };
    let oidc = Arc::new(FakeOidc::default());
    let state =
        AppState::new(cfg, store, Arc::new(FakeSlack::new()), fixed_now).with_oidc(oidc.clone());
    Setup {
        state: Arc::new(state),
        oidc,
    }
}

fn setup() -> Setup {
    setup_with(Store::default())
}

async fn send(state: &Arc<AppState>, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

fn get(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let builder = Request::get(uri);
    let builder = match cookie {
        Some(c) => builder.header(header::COOKIE, c),
        None => builder,
    };
    builder.body(Body::empty()).unwrap()
}

fn post_tokens(label: &str, cookie: Option<&str>) -> Request<Body> {
    let builder =
        Request::post("/tokens").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    let builder = match cookie {
        Some(c) => builder.header(header::COOKIE, c),
        None => builder,
    };
    let body = serde_urlencoded::to_string([("label", label)]).unwrap();
    builder.body(Body::from(body)).unwrap()
}

fn location(headers: &HeaderMap) -> &str {
    headers.get(header::LOCATION).unwrap().to_str().unwrap()
}

async fn admin_cookie(state: &AppState) -> String {
    let sid = state.sessions.lock().await.create_cookie(ADMIN, NOW);
    format!("other=1; sccr_sid={sid}")
}

fn callback_uri(code: &str, state: &str) -> String {
    format!("/auth/callback?code={code}&state={state}")
}

/// The plaintext token in an HTML body: a 43-char base64url run whose hash is stored.
fn find_token(body: &str, store: &Store) -> Option<String> {
    body.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|w| w.len() == 43)
        .find(|w| store.token_ok(w))
        .map(String::from)
}

#[tokio::test]
async fn login_redirects_to_slack_with_state() {
    let s = setup();
    let (status, headers, _) = send(&s.state, get("/login", None)).await;
    assert_eq!(status, StatusCode::FOUND);
    let loc = location(&headers);
    assert!(loc.starts_with(
        "https://slack.com/openid/connect/authorize?response_type=code&scope=openid%20profile&client_id=cid&state="
    ));
    assert!(loc.ends_with("&redirect_uri=https%3A%2F%2Fsccr.example.jp%2Fauth%2Fcallback"));
    let state = loc
        .split('&')
        .find_map(|p| p.strip_prefix("state="))
        .unwrap();
    let nonce = loc
        .split('&')
        .find_map(|p| p.strip_prefix("nonce="))
        .unwrap();
    assert_eq!(
        s.state.sessions.lock().await.take_pending(state, NOW),
        Some(nonce.to_string())
    );
}

#[tokio::test]
async fn callback_unknown_state_401() {
    let s = setup();
    s.oidc.respond(jwt(ADMIN, "n"));
    let unauthorized = (StatusCode::UNAUTHORIZED, String::new());
    for uri in [
        callback_uri("c", "bogus"),
        "/auth/callback?code=c".to_string(),
        "/auth/callback?error=access_denied&state=x".to_string(),
    ] {
        let (status, _, body) = send(&s.state, get(&uri, None)).await;
        assert_eq!((status, body), unauthorized, "{uri}");
    }

    // Expired state.
    let (state, _) = s.state.sessions.lock().await.begin(NOW - 600);
    let (status, _, _) = send(&s.state, get(&callback_uri("c", &state), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(s.oidc.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn callback_bad_nonce_401() {
    let s = setup();
    let (state, _) = s.state.sessions.lock().await.begin(NOW);
    s.oidc.respond(jwt(ADMIN, "wrong-nonce"));
    let (status, headers, body) = send(&s.state, get(&callback_uri("c", &state), None)).await;
    assert_eq!((status, body.as_str()), (StatusCode::UNAUTHORIZED, ""));
    assert!(headers.get(header::SET_COOKIE).is_none());

    // Token exchange failure is also 401.
    let s = setup();
    let (state, _) = s.state.sessions.lock().await.begin(NOW);
    let (status, _, _) = send(&s.state, get(&callback_uri("c", &state), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(s.state.store.lock().await.users.is_empty());
}

#[tokio::test]
async fn callback_non_admin_403() {
    let s = setup();
    let (state, nonce) = s.state.sessions.lock().await.begin(NOW);
    s.oidc.respond(jwt("U9999OTHER", &nonce));
    let (status, headers, _) = send(&s.state, get(&callback_uri("c", &state), None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(s.state.store.lock().await.users.is_empty());
    assert!(
        Store::load(&s.state.cfg.state_file)
            .unwrap()
            .users
            .is_empty()
    );
}

#[tokio::test]
async fn callback_ok_sets_cookie_and_registers_user() {
    let s = setup();
    let (state, nonce) = s.state.sessions.lock().await.begin(NOW);
    s.oidc.respond(jwt(ADMIN, &nonce));
    let (status, headers, _) = send(&s.state, get(&callback_uri("the-code", &state), None)).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(location(&headers), "/");

    let cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    let sid = cookie
        .strip_prefix("sccr_sid=")
        .and_then(|rest| {
            rest.strip_suffix("; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=86400")
        })
        .unwrap_or_else(|| panic!("unexpected Set-Cookie: {cookie}"));
    assert_eq!(sid.len(), 43);
    assert_eq!(
        s.state.sessions.lock().await.user_for(sid, NOW),
        Some(ADMIN.to_string())
    );

    assert_eq!(
        *s.oidc.calls.lock().unwrap(),
        vec![(
            "the-code".to_string(),
            "cid".to_string(),
            "csecret".to_string(),
            REDIRECT_URI.to_string()
        )]
    );
    assert_eq!(
        Store::load(&s.state.cfg.state_file).unwrap().users,
        vec![ADMIN.to_string()]
    );

    // The state is single-use; a repeat login does not duplicate the user.
    let (status, _, _) = send(&s.state, get(&callback_uri("the-code", &state), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (state, nonce) = s.state.sessions.lock().await.begin(NOW);
    s.oidc.respond(jwt(ADMIN, &nonce));
    let (status, _, _) = send(&s.state, get(&callback_uri("c", &state), None)).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(s.state.store.lock().await.users, vec![ADMIN.to_string()]);
}

#[tokio::test]
async fn root_without_cookie_redirects_login() {
    let s = setup();
    let expired = s
        .state
        .sessions
        .lock()
        .await
        .create_cookie(ADMIN, NOW - 86400);
    for cookie in [
        None,
        Some("sccr_sid=unknown".to_string()),
        Some(format!("sccr_sid={expired}")),
    ] {
        let (status, headers, _) = send(&s.state, get("/", cookie.as_deref())).await;
        assert_eq!(status, StatusCode::FOUND, "{cookie:?}");
        assert_eq!(location(&headers), "/login");
    }
}

#[tokio::test]
async fn root_with_cookie_lists_sessions() {
    let s = setup_with(Store {
        tokens: vec![TokenRecord {
            sha256: "ef260e9aa3c673af240d17a2660480361a8e081d1ffeca2a5ed0e3219fc18567".into(),
            label: "macbook".into(),
            created: "2026-09-16T00:00:00Z".into(),
        }],
        ..Store::default()
    });
    let (tx, _rx) = mpsc::channel(1);
    s.state.hub.register("mac:proj", tx.clone()).await.unwrap();
    s.state.hub.register("a:b", tx).await.unwrap();

    let cookie = admin_cookie(&s.state).await;
    let (status, headers, body) = send(&s.state, get("/", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "text/html; charset=utf-8"
    );
    assert!(body.contains(ADMIN));
    let (a, mac) = (body.find("a:b").unwrap(), body.find("mac:proj").unwrap());
    assert!(a < mac, "sessions must be sorted");
    assert!(body.contains(r#"<form method="post" action="/tokens">"#));
    assert!(body.contains(r#"name="label""#));
    assert!(body.contains("macbook"));
    assert!(body.contains("2026-09-16T00:00:00Z"));
    assert!(!body.contains("ef260e9a"));
}

#[tokio::test]
async fn tokens_requires_cookie() {
    let s = setup();
    for cookie in [None, Some("sccr_sid=unknown")] {
        let (status, headers, body) = send(&s.state, post_tokens("macbook", cookie)).await;
        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(location(&headers), "/login");
        assert!(body.is_empty());
    }
    assert!(s.state.store.lock().await.tokens.is_empty());
}

#[tokio::test]
async fn tokens_rejects_bad_label() {
    let s = setup();
    let cookie = admin_cookie(&s.state).await;
    for label in ["", "   ", &"x".repeat(65)] {
        let (status, _, _) = send(&s.state, post_tokens(label, Some(&cookie))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label:?}");
    }
    assert!(s.state.store.lock().await.tokens.is_empty());

    let (status, _, _) = send(&s.state, post_tokens(&" é".repeat(32), Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        s.state.store.lock().await.tokens[0].label.chars().count(),
        63
    );
}

#[tokio::test]
async fn issued_token_authenticates_ws() {
    let s = setup();
    let cookie = admin_cookie(&s.state).await;
    let (status, headers, body) = send(&s.state, post_tokens("  macbook  ", Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");

    let saved = Store::load(&s.state.cfg.state_file).unwrap();
    let token = find_token(&body, &saved).expect("plaintext token shown once");
    assert_eq!(saved.tokens.len(), 1);
    let record = &saved.tokens[0];
    assert_eq!(record.label, "macbook");
    assert_eq!(record.created, "2026-09-16T12:00:00Z");
    assert_eq!(record.sha256, hex::encode(Sha256::digest(token.as_bytes())));

    // Plaintext appears only in the POST response.
    let (_, _, page) = send(&s.state, get("/", Some(&cookie))).await;
    assert!(page.contains("macbook"));
    assert!(!page.contains(&token));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(s.state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut req = format!("ws://{addr}/agent/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (_, resp) = connect_async(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
}

#[tokio::test]
async fn html_escapes_label() {
    let s = setup();
    let (tx, _rx) = mpsc::channel(1);
    s.state.hub.register("<i>sess", tx).await.unwrap();
    let cookie = admin_cookie(&s.state).await;
    let label = r#"<script>&"'"#;
    let escaped = "&lt;script&gt;&amp;&quot;&#39;";

    let (status, _, body) = send(&s.state, post_tokens(label, Some(&cookie))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(escaped));
    assert!(!body.contains("<script>"));

    let (_, _, page) = send(&s.state, get("/", Some(&cookie))).await;
    assert!(page.contains(escaped));
    assert!(!page.contains("<script>"));
    assert!(page.contains("&lt;i&gt;sess"));
    assert!(!page.contains("<i>sess"));
}

#[test]
fn rfc3339_formats_utc() {
    assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(super::rfc3339(951782399), "2000-02-28T23:59:59Z");
    assert_eq!(super::rfc3339(1709208000), "2024-02-29T12:00:00Z");
    assert_eq!(super::rfc3339(NOW), "2026-09-16T12:00:00Z");
}
