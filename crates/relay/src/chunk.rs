//! Split long Slack reply text into chunks that fit chat.postMessage limits.
//! See spec section 6.3.1.

/// Split `text` into pieces of at most `max_chars` characters each.
///
/// Counts by `char`, not bytes. When a piece would exceed `max_chars`, the
/// cut point is the last newline within the first `max_chars` characters
/// (newline kept at the end of the piece); if there is no newline, the cut
/// is a hard split at exactly `max_chars` characters. Concatenating the
/// result always reproduces `text`. An empty string returns `vec![""]`.
pub fn chunk(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let chars: Vec<char> = text.chars().collect();
    let mut result = Vec::new();
    let mut start = 0usize;

    while chars.len() - start > max_chars {
        let window = &chars[start..start + max_chars];
        let split_len = window
            .iter()
            .rposition(|&c| c == '\n')
            .map(|i| i + 1)
            .unwrap_or(max_chars);
        result.push(chars[start..start + split_len].iter().collect());
        start += split_len;
    }
    result.push(chars[start..].iter().collect());
    result
}

#[cfg(test)]
mod tests;
