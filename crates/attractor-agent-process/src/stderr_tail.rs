//! The end of an agent's stderr, short enough for an error message.
//! A pure function.

/// How many lines [`stderr_tail`] keeps.
pub const STDERR_TAIL_LINES: usize = 10;
/// The most bytes [`stderr_tail`] keeps.
pub const STDERR_TAIL_BYTES: usize = 2000;

/// The last [`STDERR_TAIL_LINES`] lines of `stderr`, trailing blank space
/// trimmed, then at most [`STDERR_TAIL_BYTES`] bytes from its end, cut on a
/// char boundary (so possibly a little fewer bytes).
pub fn stderr_tail(stderr: &str) -> String {
    let trimmed = stderr.trim_end();
    let lines_start = trimmed
        .rmatch_indices('\n')
        .nth(STDERR_TAIL_LINES - 1)
        .map_or(0, |(newline, _)| newline + 1);
    let lines = &trimmed[lines_start..];
    let bytes_start = (lines.len().saturating_sub(STDERR_TAIL_BYTES)..lines.len())
        .find(|&index| lines.is_char_boundary(index))
        .unwrap_or(lines.len());
    lines[bytes_start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stderr_is_an_empty_tail() {
        assert_eq!(stderr_tail(""), "");
        assert_eq!(stderr_tail("\n\n  \n"), "");
    }

    #[test]
    fn a_short_stderr_is_kept_whole_without_the_trailing_newline() {
        assert_eq!(stderr_tail("one\ntwo\n"), "one\ntwo");
    }

    #[test]
    fn more_than_ten_lines_keeps_the_last_ten() {
        let stderr: String = (1..=15).map(|n| format!("line {n}\n")).collect();
        let expected: Vec<String> = (6..=15).map(|n| format!("line {n}")).collect();
        assert_eq!(stderr_tail(&stderr), expected.join("\n"));
    }

    #[test]
    fn over_2000_bytes_keeps_the_last_2000() {
        let stderr = format!("{}{}", "a".repeat(10), "b".repeat(2000));
        assert_eq!(stderr_tail(&stderr), "b".repeat(2000));
    }

    #[test]
    fn a_multibyte_char_at_the_cut_is_dropped_whole() {
        // "é" is 2 bytes; the 2000-byte cut falls inside it.
        let stderr = format!("é{}", "b".repeat(1999));
        let tail = stderr_tail(&stderr);
        assert_eq!(tail, "b".repeat(1999));
        assert!(tail.len() <= STDERR_TAIL_BYTES);
    }

    #[test]
    fn a_multibyte_char_just_inside_the_cut_is_kept() {
        let stderr = format!("xé{}", "b".repeat(1998));
        assert_eq!(stderr_tail(&stderr), format!("é{}", "b".repeat(1998)));
    }
}
