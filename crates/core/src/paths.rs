use crate::{Error, Result};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_ASSET_BYTES: u64 = 8 * 1024 * 1024;

pub fn excluded(name: &str) -> bool {
    matches!(name, ".git" | "node_modules" | "target")
}

pub fn markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"))
}

pub fn relative(path: &str) -> Result<PathBuf> {
    if path.is_empty()
        || path.contains('\\')
        || path.contains('\0')
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(Error::UnsafePath(path.into()));
    }
    let path_buf = PathBuf::from(path);
    for component in path_buf.components() {
        match component {
            Component::Normal(name) if !excluded(&name.to_string_lossy()) => {}

            _ => return Err(Error::UnsafePath(path.into())),
        }
    }
    Ok(path_buf)
}

pub fn safe_file(root: &Path, path: &str) -> Result<PathBuf> {
    let relative = relative(path)?;
    let canonical = root.join(relative).canonicalize()?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        return Err(Error::UnsafePath(path.into()));
    }
    // Reject aliases into excluded directories as well as direct access.
    let inside = canonical
        .strip_prefix(root)
        .map_err(|_| Error::UnsafePath(path.into()))?;
    relative_path(inside)?;
    Ok(canonical)
}

pub fn relative_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(Error::UnsafePath(path.display().to_string()));
        };
        parts.push(
            name.to_str()
                .ok_or_else(|| Error::UnsafePath("non-UTF-8 path".into()))?,
        );
    }
    // Native Windows separators are structural components, not user input. Keep
    // relative() strict so literal backslashes in user paths (or Unix names) fail.
    let text = parts.join("/");
    relative(&text)?;
    Ok(text)
}

pub fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(Error::TooLarge(limit));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::TooLarge(limit));
    }
    Ok(bytes)
}

pub fn asset_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        // SVG is deliberately excluded: it can contain scripts/external resources.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_relative_paths_use_slash_separators() {
        let native = PathBuf::from("nested").join("café").join("doc.md");
        assert_eq!(relative_path(&native).unwrap(), "nested/café/doc.md");
        assert!(relative("nested\\doc.md").is_err());
        for path in [
            "",
            ".",
            "..",
            "../doc.md",
            "nested/../doc.md",
            "target/doc.md",
            "node_modules/doc.md",
            ".git/doc.md",
        ] {
            assert!(relative_path(Path::new(path)).is_err(), "{path}");
        }
        let absolute = tempfile::tempdir().unwrap();
        assert!(relative_path(absolute.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_separators_are_normalized_but_roots_and_prefixes_fail() {
        assert_eq!(
            relative_path(Path::new(r"nested\notes\doc.md")).unwrap(),
            "nested/notes/doc.md"
        );
        assert!(relative(r"nested\notes\doc.md").is_err());
        for path in [
            r"\doc.md",
            r"C:\doc.md",
            r"C:doc.md",
            r"\\server\share\doc.md",
            r"\\?\C:\doc.md",
            r"\\.\C:\doc.md",
        ] {
            assert!(relative_path(Path::new(path)).is_err(), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_literal_backslashes_and_non_utf8_names_remain_rejected() {
        use std::os::unix::ffi::OsStringExt;
        assert!(relative_path(Path::new(r"nested\doc.md")).is_err());
        let invalid = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(relative_path(&invalid).is_err());
    }
}
