//! Artifact path rules. Paths are relative, `/`-separated, and restricted to a
//! conservative ASCII character set so that installation is safe and portable
//! on case-insensitive, normalization-insensitive filesystems (APFS default).

use std::collections::HashSet;

use anyhow::{Result, bail, ensure};

pub const MAX_PATH_LEN: usize = 1024;
pub const MAX_COMPONENT_LEN: usize = 255;
pub const MAX_DEPTH: usize = 32;

fn allowed_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '~' | '@' | '=' | ',')
}

/// Validate one artifact path.
pub fn validate_path(path: &str) -> Result<()> {
    ensure!(!path.is_empty(), "empty artifact path");
    ensure!(
        path.len() <= MAX_PATH_LEN,
        "artifact path too long: {path:?}"
    );
    let mut depth = 0;
    for component in path.split('/') {
        depth += 1;
        ensure!(
            !component.is_empty(),
            "empty component (absolute path, trailing or doubled '/'): {path:?}"
        );
        ensure!(
            component != "." && component != "..",
            "'.' or '..' component: {path:?}"
        );
        ensure!(
            component.len() <= MAX_COMPONENT_LEN,
            "path component too long: {path:?}"
        );
        if let Some(c) = component.chars().find(|c| !allowed_char(*c)) {
            bail!("disallowed character {c:?} in artifact path {path:?}");
        }
    }
    ensure!(depth <= MAX_DEPTH, "artifact path too deep: {path:?}");
    Ok(())
}

/// Validate a full set of paths, which must already be sorted strictly
/// ascending by byte value. Rejects case-insensitive duplicates and paths that
/// are used both as a file and as a directory.
pub fn validate_path_set<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut prev: Option<&str> = None;
    let mut files = HashSet::new();
    let mut dirs = HashSet::new();
    for path in paths {
        validate_path(path)?;
        if let Some(prev) = prev {
            ensure!(
                prev < path,
                "artifact paths not strictly sorted: {prev:?} then {path:?}"
            );
        }
        prev = Some(path);
        let folded = path.to_ascii_lowercase();
        ensure!(
            !dirs.contains(&folded),
            "path {path:?} is also used as a directory"
        );
        let mut idx = 0;
        while let Some(pos) = folded[idx..].find('/') {
            let dir = folded[..idx + pos].to_string();
            ensure!(!files.contains(&dir), "path {path:?} is inside a file path");
            dirs.insert(dir);
            idx += pos + 1;
        }
        ensure!(
            files.insert(folded),
            "case-insensitive duplicate artifact path {path:?}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_paths() {
        for p in [
            "a",
            "venues.csv",
            "2026/09/events.json",
            "x-1_y+z~@=,.bin",
            ".hidden",
        ] {
            validate_path(p).unwrap();
        }
    }

    #[test]
    fn rejects_unsafe_paths() {
        for p in [
            "",
            "/abs",
            "a/../b",
            "..",
            ".",
            "./a",
            "a//b",
            "a/",
            "a\\b",
            "a\0b",
            "caf\u{e9}",
            "sp ace",
            "a:b",
            "C:x",
        ] {
            assert!(validate_path(p).is_err(), "accepted {p:?}");
        }
        assert!(validate_path(&"a".repeat(MAX_COMPONENT_LEN + 1)).is_err());
        assert!(validate_path(&vec!["a"; MAX_DEPTH + 1].join("/")).is_err());
    }

    #[test]
    fn rejects_conflicting_sets() {
        assert!(validate_path_set(["A.txt", "a.txt"]).is_err());
        assert!(validate_path_set(["a", "a/b"]).is_err());
        assert!(validate_path_set(["A", "a/b"]).is_err());
        assert!(validate_path_set(["a/b", "a/b"]).is_err());
        assert!(validate_path_set(["b", "a"]).is_err());
        validate_path_set(["a/b", "a/c", "b"]).unwrap();
    }
}
