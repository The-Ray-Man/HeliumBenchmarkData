//! Line counts of a source file, for any language with C-style comments
//! (Rust and Viper both use `//` and `/* */`).

/// Size of a source text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TextMetrics {
    /// Every line, including blank and comment-only ones.
    pub lines: u64,
    /// Lines of code: lines with anything besides whitespace and comments.
    pub loc: u64,
    pub bytes: u64,
}

impl TextMetrics {
    pub fn of(source: &str) -> Self {
        TextMetrics {
            lines: source.lines().count() as u64,
            loc: count_loc(source),
            bytes: source.len() as u64,
        }
    }
}

/// Lines with code outside `//` and `/* */` comments. Block comments may span
/// lines and nest (as in Rust). String literals are not tracked, so a `/*` or
/// `//` inside a string is taken as a comment start; the benchmark files do
/// not rely on that.
pub fn count_loc(source: &str) -> u64 {
    let mut depth = 0usize; // open block comments
    let mut loc = 0;
    for line in source.lines() {
        let mut code = false;
        let mut rest = line;
        while !rest.is_empty() {
            if depth > 0 {
                match (rest.find("/*"), rest.find("*/")) {
                    (Some(open), close) if close.is_none_or(|c| open < c) => {
                        depth += 1;
                        rest = &rest[open + 2..];
                    }
                    (_, Some(close)) => {
                        depth -= 1;
                        rest = &rest[close + 2..];
                    }
                    _ => break,
                }
            } else {
                let trimmed = rest.trim_start();
                if trimmed.starts_with("//") || trimmed.is_empty() {
                    break;
                }
                if let Some(after) = trimmed.strip_prefix("/*") {
                    depth = 1;
                    rest = after;
                    continue;
                }
                code = true;
                // Code on this line; only a block comment opening later matters.
                match (trimmed.find("/*"), trimmed.find("//")) {
                    (Some(open), line_comment) if line_comment.is_none_or(|l| open < l) => {
                        depth = 1;
                        rest = &trimmed[open + 2..];
                    }
                    _ => break,
                }
            }
        }
        loc += u64::from(code);
    }
    loc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_loc_and_bytes() {
        let src = "fn f() {\n\n    // comment\n    let x = 1; // trailing\n}\n";
        assert_eq!(
            TextMetrics::of(src),
            TextMetrics { lines: 5, loc: 3, bytes: src.len() as u64 }
        );
    }

    #[test]
    fn skips_block_comments() {
        let src = "/* header\n   still header */\nmethod m()\n/* a */ /* b */\nx /* c\n d */ y\n*/ not closed above";
        // `method m()`, `x /* c`, ` d */ y`, `*/ not closed above` (no comment open there).
        assert_eq!(count_loc(src), 4);
    }

    #[test]
    fn handles_nested_block_comments() {
        let src = "/* outer /* inner */\nstill comment */\ncode";
        assert_eq!(count_loc(src), 1);
    }

    #[test]
    fn code_after_closing_comment_counts() {
        assert_eq!(count_loc("/* c */ code"), 1);
        assert_eq!(count_loc("/* c\n*/ code"), 1);
        assert_eq!(count_loc("/* c\n*/ // only comment"), 0);
    }
}
