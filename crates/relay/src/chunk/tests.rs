use super::chunk;

#[test]
fn short_single() {
    assert_eq!(chunk("hello", 10), vec!["hello"]);
}

#[test]
fn exact_max_single() {
    assert_eq!(chunk("hello", 5), vec!["hello"]);
}

#[test]
fn splits_on_last_newline() {
    assert_eq!(chunk("ab\ncd\nef", 6), vec!["ab\ncd\n", "ef"]);
}

#[test]
fn hard_split_when_no_newline() {
    assert_eq!(chunk("abcdefgh", 3), vec!["abc", "def", "gh"]);
}

#[test]
fn multibyte_safe() {
    let s = "あいうえおかきくけこ";
    assert_eq!(s.chars().count(), 10);
    let parts = chunk(s, 3);
    assert_eq!(parts.len(), 4);
    assert_eq!(parts.concat(), s);
}

#[test]
fn concat_equals_original() {
    let cases: &[(&str, usize)] = &[
        ("abcdefgh", 3),
        ("ab\ncd\nef", 6),
        ("hello", 10),
        ("hello", 5),
        ("", 5),
        ("line1\nline2\nline3\n", 7),
    ];
    for (text, max) in cases {
        let parts = chunk(text, *max);
        assert_eq!(parts.concat(), *text, "failed for {text:?} max {max}");
    }
}

#[test]
fn empty_gives_one_empty() {
    assert_eq!(chunk("", 10), vec![""]);
}
