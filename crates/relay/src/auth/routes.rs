//! Login and admin routes (spec 9, 10). Codes, id_tokens, cookies and agent
//! tokens are never logged.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::html;
use super::oidc::{authorize_url, parse_id_token};
use super::session::random_token;
use crate::state::{AppState, save_store};
use crate::store::{Store, TokenRecord};

const COOKIE_NAME: &str = "sccr_sid";
const COOKIE_ATTRS: &str = "HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=86400";
const MAX_LABEL_CHARS: usize = 64;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/", get(root))
        .route("/tokens", post(tokens))
}

fn found(location: &str) -> Response {
    (
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response()
}

fn redirect_uri(state: &AppState) -> String {
    format!("{}/auth/callback", state.cfg.public_url)
}

/// Applies `change` and saves the state file. Memory is left untouched when
/// saving fails.
async fn commit(state: &AppState, change: impl FnOnce(&mut Store)) -> bool {
    let mut store = state.store.lock().await;
    let mut next = store.clone();
    change(&mut next);
    if next == *store {
        return true;
    }
    if !save_store(state, &next) {
        return false;
    }
    *store = next;
    true
}

/// The logged-in user behind a valid `sccr_sid` cookie.
async fn current_user(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let sid = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix("sccr_sid="))?;
    state.sessions.lock().await.user_for(sid, (state.now)())
}

async fn login(State(state): State<Arc<AppState>>) -> Response {
    let (st, nonce) = state.sessions.lock().await.begin((state.now)());
    found(&authorize_url(
        &state.cfg.client_id,
        &redirect_uri(&state),
        &st,
        &nonce,
    ))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// Spec 9.2.
async fn callback(State(state): State<Arc<AppState>>, RawQuery(query): RawQuery) -> Response {
    let unauthorized = StatusCode::UNAUTHORIZED.into_response();
    let query = query
        .as_deref()
        .and_then(|q| serde_urlencoded::from_str::<CallbackQuery>(q).ok());
    let Some(CallbackQuery {
        code: Some(code),
        state: Some(login_state),
    }) = query
    else {
        return unauthorized;
    };
    let now = (state.now)();
    let Some(nonce) = state.sessions.lock().await.take_pending(&login_state, now) else {
        return unauthorized;
    };
    let cfg = &state.cfg;
    let Ok(id_token) = state
        .oidc
        .token(
            &code,
            &cfg.client_id,
            &cfg.client_secret,
            &redirect_uri(&state),
        )
        .await
    else {
        eprintln!("login: token exchange failed");
        return unauthorized;
    };
    let Ok(claims) = parse_id_token(&id_token, &cfg.client_id, &nonce, now) else {
        eprintln!("login: id_token rejected");
        return unauthorized;
    };
    if cfg.admin_user.is_empty() || claims.sub != cfg.admin_user {
        eprintln!("login: rejected non-admin user");
        return StatusCode::FORBIDDEN.into_response();
    }

    let sub = claims.sub;
    let saved = commit(&state, |store| {
        if !store.users.contains(&sub) {
            store.users.push(sub.clone());
        }
    })
    .await;
    if !saved {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let sid = state.sessions.lock().await.create_cookie(&sub, now);
    eprintln!("login: admin signed in");
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, "/".to_string()),
            (
                header::SET_COOKIE,
                format!("{COOKIE_NAME}={sid}; {COOKIE_ATTRS}"),
            ),
        ],
    )
        .into_response()
}

/// Spec 10.1.
async fn root(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(user) = current_user(&state, &headers).await else {
        return found("/login");
    };
    let sessions = state.hub.list().await;
    let tokens = state.store.lock().await.tokens.clone();
    Html(html::admin_page(&user, &sessions, &tokens)).into_response()
}

#[derive(Deserialize)]
struct TokenForm {
    label: String,
}

/// Spec 10.2.
async fn tokens(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    if current_user(&state, &headers).await.is_none() {
        return found("/login");
    }
    let label = serde_urlencoded::from_bytes::<TokenForm>(&body)
        .ok()
        .map(|form| form.label.trim().to_string())
        .filter(|label| (1..=MAX_LABEL_CHARS).contains(&label.chars().count()));
    let Some(label) = label else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let token = random_token();
    let record = TokenRecord {
        sha256: hex::encode(Sha256::digest(token.as_bytes())),
        label,
        created: rfc3339((state.now)()),
    };
    let label = record.label.clone();
    if !commit(&state, |store| store.tokens.push(record)).await {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    eprintln!("token issued");
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(html::token_page(&label, &token)),
    )
        .into_response()
}

/// Unix seconds as RFC 3339 UTC with second precision (civil-from-days).
fn rfc3339(unix: i64) -> String {
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests;
