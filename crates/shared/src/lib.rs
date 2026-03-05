pub mod types;
pub mod errors;
pub mod events;
pub mod channel;
pub mod bus;

pub fn strip_code_fences(s: &str) -> String {
    let s = s.trim();

    if let Some(start) = s.find('{') {
        if let Some(end) = s.rfind('}') {
            if end >= start {
                return s[start..=end].to_string();
            }
        }
    }

    let s = if s.starts_with("```") {
        let after = s.trim_start_matches('`');
        if let Some(newline) = after.find('\n') {
            after[newline + 1..].trim()
        } else {
            after.trim()
        }
    } else {
        s
    };

    let s = if s.ends_with("```") {
        s[..s.len() - 3].trim()
    } else {
        s
    };

    s.to_string()
}

pub fn strip_frontmatter(s: &str) -> &str {
    if s.starts_with("---") {
        if let Some(end) = s[3..].find("---") {
            return s[3 + end + 3..].trim_start();
        }
    }
    s
}