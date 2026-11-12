//! Path globs for tally checks: `*` within a path segment, `**` across segments, `?` for one
//! character. Paths are compared with forward slashes on every platform.

/// Whether `path` matches `pattern`.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    let pattern = pattern.trim_start_matches("./");
    let path = path.trim_start_matches("./");
    // A pattern naming a directory covers everything below it.
    if let Some(dir) = pattern.strip_suffix('/') {
        return path == dir || path.starts_with(&format!("{dir}/"));
    }
    let pat: Vec<&str> = pattern.split('/').collect();
    let segs: Vec<&str> = path.split('/').collect();
    match_segments(&pat, &segs)
}

fn match_segments(pat: &[&str], segs: &[&str]) -> bool {
    match pat.first() {
        None => segs.is_empty(),
        Some(&"**") => {
            if pat.len() == 1 {
                return true;
            }
            (0..=segs.len()).any(|skip| match_segments(&pat[1..], &segs[skip..]))
        }
        Some(p) => match segs.first() {
            Some(s) if match_segment(p.as_bytes(), s.as_bytes()) => match_segments(&pat[1..], &segs[1..]),
            _ => false,
        },
    }
}

fn match_segment(pat: &[u8], text: &[u8]) -> bool {
    match pat.first() {
        None => text.is_empty(),
        Some(b'*') => (0..=text.len()).any(|skip| match_segment(&pat[1..], &text[skip..])),
        Some(b'?') => !text.is_empty() && match_segment(&pat[1..], &text[1..]),
        Some(c) => text.first() == Some(c) && match_segment(&pat[1..], &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn globs_behave_like_gitignore_style_patterns() {
        assert!(matches("src/auth/**", "src/auth/refresh.ts"));
        assert!(matches("src/auth/**", "src/auth/deep/x.ts"));
        assert!(!matches("src/auth/**", "src/other/x.ts"));
        assert!(matches("**/*.key", "a/b/c.key"));
        assert!(matches("**/*.key", "c.key"));
        assert!(matches("*.md", "README.md"));
        assert!(!matches("*.md", "doc/README.md"));
        assert!(matches("package.json", "package.json"));
        assert!(matches("src/", "src/a/b.rs"));
        assert!(matches("test/?.txt", "test/a.txt"));
        assert!(!matches("test/?.txt", "test/ab.txt"));
        assert!(matches("src\\auth\\**", "src/auth/x.ts"));
        assert!(matches("**", "anything/at/all"));
    }
}
