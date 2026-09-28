//! Bounded file boundary for the host's `message_send` node.
use super::StepError;
use crate::agent_messaging::MAX_MESSAGE_BYTES;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

fn failure(error: impl std::fmt::Display) -> StepError {
    StepError::new(format!("message_send artifact: {error}"))
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Includes junctions and other reparse points, not just symbolic links.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn artifact_path(workspace: &Path, relative: &str, prepare: bool) -> Result<PathBuf, StepError> {
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.contains([':', '\\'])
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(failure(
            "expected a workspace-relative path without traversal",
        ));
    }
    let root = workspace.canonicalize().map_err(failure)?;
    let mut target = root.clone();
    let parts: Vec<_> = path.components().collect();
    for (index, part) in parts.iter().enumerate() {
        target.push(part.as_os_str());
        let parent = index + 1 < parts.len();
        match std::fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if is_link(&metadata) || (parent && !metadata.is_dir()) {
                    return Err(failure(
                        "links, reparse points and non-directory parents are forbidden",
                    ));
                }
                if !target.canonicalize().map_err(failure)?.starts_with(&root) {
                    return Err(failure("path escapes workspace"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && parent && prepare => {
                std::fs::create_dir(&target).map_err(failure)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !parent => {}
            Err(error) => return Err(failure(error)),
        }
    }
    Ok(target)
}

/// Prepare only this file's directory, rejecting stale fresh-run artifacts.
pub fn prepare(workspace: &Path, relative: &str, fresh: bool) -> Result<(), StepError> {
    let path = artifact_path(workspace, relative, true)?;
    if fresh && path.try_exists().map_err(failure)? {
        return Err(failure("artifact already exists on a fresh run"));
    }
    Ok(())
}

/// Read at most the messaging limit plus one byte, once, with strict UTF-8.
/// Hash and body share exactly the same bytes (including BOM and CRLF).
pub fn read(workspace: &Path, relative: &str) -> Result<(String, String), StepError> {
    let path = artifact_path(workspace, relative, false)?;
    let file = std::fs::File::open(path).map_err(failure)?;
    if !file.metadata().map_err(failure)?.is_file() {
        return Err(failure("artifact must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    let body = String::from_utf8(bytes).map_err(failure)?;
    crate::db::agent_messaging::validate_message(&body).map_err(failure)?;
    let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
    Ok((body, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_utf8_crlf_bom_and_metacharacters_are_data() {
        let dir = tempfile::tempdir().unwrap();
        let body = "\u{feff}Review: café 日本語 🦀\r\n'quotes' \"double\" `$() & | ; < > {{run.id}}\r\nVerdict: blocked\r\n";
        prepare(dir.path(), "run/review.md", true).unwrap();
        std::fs::write(dir.path().join("run/review.md"), body).unwrap();
        let (actual, hash) = read(dir.path(), "run/review.md").unwrap();
        assert_eq!(actual.as_bytes(), body.as_bytes());
        assert_eq!(hash, format!("{:x}", Sha256::digest(body.as_bytes())));
        assert!(prepare(dir.path(), "run/review.md", true).is_err());
        prepare(dir.path(), "run/review.md", false).unwrap();
    }

    #[test]
    fn rejects_missing_invalid_empty_oversize_and_directory_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path(), "absent.md").is_err());
        for bytes in [
            vec![],
            b" \r\n\t".to_vec(),
            vec![0xff],
            vec![b'x'; MAX_MESSAGE_BYTES + 1],
        ] {
            std::fs::write(dir.path().join("review.md"), bytes).unwrap();
            assert!(read(dir.path(), "review.md").is_err());
        }
        std::fs::write(dir.path().join("review.md"), vec![b'x'; MAX_MESSAGE_BYTES]).unwrap();
        assert!(read(dir.path(), "review.md").is_ok());
        std::fs::create_dir(dir.path().join("directory")).unwrap();
        assert!(read(dir.path(), "directory").is_err());
    }

    #[test]
    fn rejects_escape_absolute_ads_and_foreign_platform_paths() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "",
            "../review.md",
            "inside/../../review.md",
            "/review.md",
            "C:/review.md",
            "review.md:stream",
            "..\\review.md",
            "\\\\host\\review.md",
        ] {
            assert!(prepare(dir.path(), path, true).is_err(), "{path}");
            assert!(read(dir.path(), path).is_err(), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_artifacts_and_parents_even_inside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source"), "another run").unwrap();
        std::os::unix::fs::symlink(dir.path().join("source"), dir.path().join("alias")).unwrap();
        assert!(read(dir.path(), "alias").is_err());
        std::os::unix::fs::symlink(dir.path(), dir.path().join("linked")).unwrap();
        assert!(prepare(dir.path(), "linked/review.md", true).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_unreadable_exclusively_open_artifact() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.md");
        std::fs::write(&path, "review").unwrap();
        let _exclusive = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(read(dir.path(), "review.md").is_err());
    }
}
