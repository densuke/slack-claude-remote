//! Admin page HTML (spec 10). Every dynamic value goes through `escape`.

use crate::store::TokenRecord;

const HEAD: &str =
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>sccr</title></head><body>";
const TAIL: &str = "</body></html>";

pub fn escape(value: &str) -> String {
    value
        .chars()
        .fold(String::with_capacity(value.len()), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                _ => out.push(c),
            }
            out
        })
}

/// `GET /` (spec 10.1).
pub fn admin_page(user: &str, sessions: &[String], tokens: &[TokenRecord]) -> String {
    let sessions_html = if sessions.is_empty() {
        "<p>No sessions connected.</p>".to_string()
    } else {
        let items: String = sessions
            .iter()
            .map(|name| format!("<li>{}</li>", escape(name)))
            .collect();
        format!("<ul>{items}</ul>")
    };
    let tokens_html = if tokens.is_empty() {
        "<p>No tokens issued.</p>".to_string()
    } else {
        let rows: String = tokens
            .iter()
            .map(|t| {
                format!(
                    "<tr><td>{}</td><td>{}</td></tr>",
                    escape(&t.label),
                    escape(&t.created)
                )
            })
            .collect();
        format!("<table><tr><th>Label</th><th>Created</th></tr>{rows}</table>")
    };
    format!(
        "{HEAD}<p>Signed in as {}</p>\
         <h2>Connected sessions</h2>{sessions_html}\
         <h2>Issue agent token</h2>\
         <form method=\"post\" action=\"/tokens\"><input name=\"label\" maxlength=\"64\" required> <button type=\"submit\">Issue</button></form>\
         <h2>Issued tokens</h2>{tokens_html}{TAIL}",
        escape(user)
    )
}

/// `POST /tokens` success (spec 10.2). The only page that shows a plaintext token.
pub fn token_page(label: &str, token: &str) -> String {
    format!(
        "{HEAD}<p>Token for {} (shown only once):</p><pre>{}</pre><p><a href=\"/\">Back</a></p>{TAIL}",
        escape(label),
        escape(token)
    )
}
