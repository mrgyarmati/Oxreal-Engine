//! Parsing `slangc -depfile` output (Make syntax).
//!
//! `slangc` writes `target: dep1 dep2 ...`. Spaces and colons inside paths are escaped with a
//! backslash (`C\:\\Users\\Peter\ Gyarmati\\...`), and a backslash at the end of a line
//! continues it.

use std::path::PathBuf;

/// The dependencies listed in a depfile, in order, without duplicates.
///
/// # Errors
///
/// A description of the problem if there's no `target:` separator.
pub(crate) fn parse(text: &str) -> Result<Vec<PathBuf>, String> {
    let tokens = tokenize(text);
    let separator = tokens
        .iter()
        .position(|t| t.ends_with(':') && !t.ends_with("\\:"))
        .ok_or("depfile has no `target:` separator")?;
    let mut deps: Vec<PathBuf> = Vec::new();
    for token in &tokens[separator + 1..] {
        let path = PathBuf::from(unescape(token));
        if !deps.contains(&path) {
            deps.push(path);
        }
    }
    Ok(deps)
}

/// Splits on unescaped whitespace, keeping escapes for `unescape`. Line continuations
/// (`\` + newline) count as whitespace.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some('\n' | '\r') => {
                    chars.next();
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                Some(&next) => {
                    current.push('\\');
                    current.push(next);
                    chars.next();
                }
                None => current.push('\\'),
            },
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Resolves `\ `, `\:`, `\\` and `\#` escapes.
fn unescape(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next @ (' ' | ':' | '\\' | '#')) => out.push(next),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_slangc_output_on_windows() {
        // Captured from slangc 2026.17 during the ADR-0013 spike.
        let text = r"mesh.spv: C\:\\Users\\Peter\ Gyarmati\\scratch\\mesh.slang C\:\\Users\\Peter\ Gyarmati\\scratch\\common.slang";
        assert_eq!(
            parse(text).unwrap(),
            vec![
                PathBuf::from(r"C:\Users\Peter Gyarmati\scratch\mesh.slang"),
                PathBuf::from(r"C:\Users\Peter Gyarmati\scratch\common.slang"),
            ]
        );
    }

    #[test]
    fn parses_unix_paths_continuations_and_duplicates() {
        let text = "out.spv: /src/a.slang \\\n  /src/b\\ c.slang /src/a.slang\n";
        assert_eq!(
            parse(text).unwrap(),
            vec![
                PathBuf::from("/src/a.slang"),
                PathBuf::from("/src/b c.slang")
            ]
        );
    }

    #[test]
    fn missing_separator_is_an_error() {
        assert!(parse("just some words").is_err());
        assert!(parse("").is_err());
    }
}
