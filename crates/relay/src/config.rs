//! Relay configuration from environment variables (README 4.4, spec 12).

use std::path::PathBuf;

/// No `Debug`: holds secrets.
#[derive(Clone)]
pub struct Config {
    pub bind: String,
    pub public_url: String,
    pub state_file: PathBuf,
    pub admin_user: String,
    pub signing_secret: String,
    pub bot_token: String,
    pub client_id: String,
    pub client_secret: String,
    pub dev: bool,
}

/// Reads the environment. Outside dev mode, Slack secrets, the admin user and
/// the public URL are required.
pub fn from_env() -> Result<Config, String> {
    let var = |key: &str| std::env::var(key).unwrap_or_default();
    let dev = var("SCCR_DEV") == "1";
    let required = |key: &str| {
        let value = var(key);
        if value.is_empty() && !dev {
            Err(format!("{key} is not set"))
        } else {
            Ok(value)
        }
    };
    let or_default = |key: &str, default: &str| {
        let value = var(key);
        if value.is_empty() {
            default.to_string()
        } else {
            value
        }
    };

    Ok(Config {
        bind: or_default("SCCR_BIND", "127.0.0.1:8080"),
        public_url: required("SCCR_PUBLIC_URL")?,
        state_file: PathBuf::from(or_default("SCCR_STATE_FILE", "./sccr-state.json")),
        admin_user: required("SCCR_ADMIN_SLACK_USER")?,
        signing_secret: required("SLACK_SIGNING_SECRET")?,
        bot_token: required("SLACK_BOT_TOKEN")?,
        client_id: required("SLACK_CLIENT_ID")?,
        client_secret: required("SLACK_CLIENT_SECRET")?,
        dev,
    })
}
