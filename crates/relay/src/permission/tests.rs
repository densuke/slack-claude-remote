use std::collections::HashMap;

use protocol::Behavior;

use super::*;

#[test]
fn yes_parsed_allow() {
    assert_eq!(
        parse_verdict("yes abcde"),
        Some(("abcde".into(), Behavior::Allow))
    );
}

#[test]
fn y_parsed_allow() {
    assert_eq!(
        parse_verdict("y abcde"),
        Some(("abcde".into(), Behavior::Allow))
    );
}

#[test]
fn no_parsed_deny() {
    assert_eq!(
        parse_verdict("no abcde"),
        Some(("abcde".into(), Behavior::Deny))
    );
    assert_eq!(
        parse_verdict("n abcde"),
        Some(("abcde".into(), Behavior::Deny))
    );
}

#[test]
fn uppercase_accepted() {
    assert_eq!(
        parse_verdict("YES ABCDE"),
        Some(("abcde".into(), Behavior::Allow))
    );
}

#[test]
fn surrounding_whitespace_ok() {
    assert_eq!(
        parse_verdict("  YES ABCDE  "),
        Some(("abcde".into(), Behavior::Allow))
    );
}

#[test]
fn letter_l_rejected() {
    assert_eq!(parse_verdict("yes abcle"), None);
    assert_eq!(parse_verdict("yes abcLe"), None);
}

#[test]
fn four_letters_rejected() {
    assert_eq!(parse_verdict("yes abcd"), None);
}

#[test]
fn extra_words_rejected() {
    assert_eq!(parse_verdict("yes abcde please"), None);
}

#[test]
fn plain_text_none() {
    assert_eq!(parse_verdict("yeah abcde"), None);
    assert_eq!(parse_verdict("hello"), None);
}

#[test]
fn prompt_contains_id_and_tool() {
    let text = prompt_text(
        "Bash",
        "List files in the project root",
        "{\"command\": \"ls -la\"}",
        "abcde",
    );
    assert_eq!(
        text,
        "Claude wants to run *Bash*: List files in the project root\n\
         ```{\"command\": \"ls -la\"}```\n\
         Reply \"yes abcde\" or \"no abcde\" in this thread."
    );
}

#[test]
fn prompt_escapes_mrkdwn_and_allows_empty_preview() {
    let text = prompt_text("A&B", "<x>", "", "abcde");
    assert_eq!(
        text,
        "Claude wants to run *A&amp;B*: &lt;x&gt;\n``````\nReply \"yes abcde\" or \"no abcde\" in this thread."
    );
}

#[test]
fn sweep_drops_old_only() {
    let mut pending = HashMap::new();
    pending.insert(
        "aaaaa".to_string(),
        Pending {
            chat_id: "C:1".into(),
            session: "s".into(),
            created: 1000,
        },
    );
    pending.insert(
        "bbbbb".to_string(),
        Pending {
            chat_id: "C:2".into(),
            session: "s".into(),
            created: 999,
        },
    );
    // now - created == 1800 for aaaaa (kept), 1801 for bbbbb (dropped)
    sweep(&mut pending, 2800);
    assert!(pending.contains_key("aaaaa"));
    assert!(!pending.contains_key("bbbbb"));
}
