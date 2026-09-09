//! Claude's sibling view of Wardian-owned Markdown. Never expand user imports.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path};

const MARKER: &str = "<!-- Wardian Claude projection v1; source=AGENTS.md; body_sha256=";

/// Refresh only existing bridges at the exact managed include roots.
pub(super) fn refresh_managed_roots(
    wardian_home: &Path,
    class_name: &str,
    session_id: &str,
) -> Result<(), String> {
    let mut roots = vec![wardian_home.join("common")];
    if let Some(class) = super::safe_class_dir(wardian_home, class_name) {
        roots.push(class);
    }
    // Session identities must not turn this allowlist into an arbitrary path.
    let mut components = Path::new(session_id).components();
    if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
        roots.push(wardian_home.join("agents").join(session_id));
    }
    for root in roots {
        refresh(&root, wardian_home, false)?;
    }
    Ok(())
}

/// Materialize the generated habitat after aggregation or its memory append.
pub(super) fn refresh_habitat(root: &Path) -> Result<(), String> {
    refresh(root, root, true)
}

fn refresh(root: &Path, boundary: &Path, create: bool) -> Result<(), String> {
    refresh_inner(root, boundary, create).map_err(|error| {
        format!(
            "Could not project Claude instructions at {}: {error}",
            root.display()
        )
    })
}

fn refresh_inner(root: &Path, boundary: &Path, create: bool) -> io::Result<()> {
    // Refuse linked roots and parents, including Windows junctions, even when
    // they resolve inside Wardian home. A linked directory is not ours to edit.
    let relative = root.strip_prefix(boundary).map_err(io::Error::other)?;
    let mut current = boundary.to_path_buf();
    for part in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(part) = part {
            if !matches!(part, Component::Normal(_)) {
                return Ok(());
            }
            current.push(part);
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !is_link(&metadata) => {}
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(()),
            Err(error) => return Err(error),
        }
    }

    let target = root.join("CLAUDE.md");
    let previous = match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_file() && !is_link(&metadata) => {
            if has_multiple_links(&target)? {
                return Ok(());
            }
            let bytes = fs::read(&target)?;
            if !is_owned(&bytes) {
                return Ok(());
            }
            Some(bytes)
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if !create {
                return Ok(());
            }
            None
        }
        Err(error) => return Err(error),
    };
    let source = root.join("AGENTS.md");
    let metadata = fs::symlink_metadata(&source)?;
    if !metadata.is_file() || is_link(&metadata) || has_multiple_links(&source)? {
        return Ok(());
    }
    let body = fs::read_to_string(&source)?.into_bytes();
    let mut projected = format!("{MARKER}{:x} -->\n", Sha256::digest(&body)).into_bytes();
    projected.extend_from_slice(&body);
    if previous.as_deref() == Some(projected.as_slice()) {
        return Ok(());
    }
    // Publish a complete sibling file; never truncate a canonical or linked file.
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    temporary.write_all(&projected)?;
    temporary.persist(&target).map_err(|error| error.error)?;
    Ok(())
}

fn is_owned(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if matches!(text, "@AGENTS.md" | "@AGENTS.md\n" | "@AGENTS.md\r\n") {
        return true;
    }
    let Some((header, body)) = text.split_once('\n') else {
        return false;
    };
    header == format!("{MARKER}{:x} -->", Sha256::digest(body.as_bytes()))
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & super::FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn has_multiple_links(path: &Path) -> io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        let file = fs::File::open(path)?;
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: file owns a live handle, and the API initializes info on success.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { info.assume_init() }.nNumberOfLinks > 1)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(fs::metadata(path)?.nlink() > 1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(root: &Path) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("AGENTS.md"), "# Owned instructions\n").unwrap();
        fs::write(root.join("CLAUDE.md"), "@AGENTS.md\n").unwrap();
    }

    #[test]
    fn refreshes_owned_snapshots_without_expanding_nested_imports() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        for body in ["first\n", "second\r\n@../untrusted.md\n`@inert.md`\n"] {
            fs::write(root.path().join("AGENTS.md"), body).unwrap();
            refresh_habitat(root.path()).unwrap();
            let projection = fs::read(root.path().join("CLAUDE.md")).unwrap();
            assert!(is_owned(&projection));
            assert!(projection.ends_with(body.as_bytes()));
            assert_eq!(
                fs::read(root.path().join("AGENTS.md")).unwrap(),
                body.as_bytes()
            );
            refresh_habitat(root.path()).unwrap();
            assert_eq!(fs::read(root.path().join("CLAUDE.md")).unwrap(), projection);
        }
    }

    #[test]
    fn preserves_custom_text_and_mutated_marker_or_body() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        refresh_habitat(root.path()).unwrap();
        let target = root.path().join("CLAUDE.md");
        let generated = fs::read_to_string(&target).unwrap();
        for custom in [
            "# Custom\n@AGENTS.md\n".to_string(),
            " @AGENTS.md\n".to_string(),
            generated.replace("projection v1", "projection v2"),
            generated.replace("body_sha256=", "body_sha256=0"),
            format!("{generated}\nUser addition\n"),
        ] {
            fs::write(&target, &custom).unwrap();
            fs::write(root.path().join("AGENTS.md"), "changed canonical\n").unwrap();
            refresh_habitat(root.path()).unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), custom);
        }
    }

    #[test]
    fn missing_or_unreadable_source_cannot_erase_an_owned_bridge() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let source = root.path().join("AGENTS.md");
        fs::remove_file(&source).unwrap();
        assert!(refresh_habitat(root.path()).is_err());
        fs::write(&source, [0xff]).unwrap();
        assert!(refresh_habitat(root.path()).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
    }

    #[test]
    fn only_existing_bridges_in_exact_managed_roots_are_refreshed() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "common",
            "classes/Builder",
            "agents/agent-1",
            "workspace",
            "include",
            "classes/Other",
            "agents/other",
        ] {
            seed(&root.path().join(path));
        }
        refresh_managed_roots(root.path(), "Builder", "agent-1").unwrap();
        for path in ["common", "classes/Builder", "agents/agent-1"] {
            assert!(fs::read_to_string(root.path().join(path).join("CLAUDE.md"))
                .unwrap()
                .starts_with(MARKER));
        }
        for path in ["workspace", "include", "classes/Other", "agents/other"] {
            assert_eq!(
                fs::read_to_string(root.path().join(path).join("CLAUDE.md")).unwrap(),
                "@AGENTS.md\n"
            );
        }
        let common_bridge = root.path().join("common/CLAUDE.md");
        fs::remove_file(&common_bridge).unwrap();
        refresh_managed_roots(root.path(), "../workspace", "../include").unwrap();
        assert!(!common_bridge.exists());
        assert_eq!(
            fs::read_to_string(root.path().join("include/CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
    }

    #[test]
    fn preserves_hardlinked_sources_and_targets() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        for filename in ["AGENTS.md", "CLAUDE.md"] {
            let alias = root.path().join("alias.md");
            fs::hard_link(root.path().join(filename), &alias).unwrap();
            refresh_habitat(root.path()).unwrap();
            assert_eq!(
                fs::read_to_string(root.path().join("CLAUDE.md")).unwrap(),
                "@AGENTS.md\n"
            );
            assert!(same_file::is_same_file(root.path().join(filename), &alias).unwrap());
            fs::remove_file(alias).unwrap();
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn preserves_symlinked_sources_targets_and_dangling_targets() {
        #[cfg(unix)]
        use std::os::unix::fs::symlink as symlink_file;
        #[cfg(windows)]
        use std::os::windows::fs::symlink_file;
        let root = tempfile::tempdir().unwrap();
        let external = root.path().join("external.md");
        fs::write(&external, "@AGENTS.md\n").unwrap();
        for filename in ["AGENTS.md", "CLAUDE.md"] {
            let managed = root.path().join("managed");
            seed(&managed);
            let link = managed.join(filename);
            fs::remove_file(&link).unwrap();
            symlink_file(&external, &link).expect("create real file symlink");
            refresh_habitat(&managed).unwrap();
            assert_eq!(fs::read_link(&link).unwrap(), external);
            assert_eq!(
                fs::read_to_string(managed.join("CLAUDE.md")).unwrap(),
                "@AGENTS.md\n"
            );
            fs::remove_file(&link).unwrap();
        }
        assert_eq!(fs::read_to_string(&external).unwrap(), "@AGENTS.md\n");
        let dangling = root.path().join("managed/CLAUDE.md");
        symlink_file(root.path().join("absent.md"), &dangling).unwrap();
        refresh_habitat(&root.path().join("managed")).unwrap();
        assert!(fs::symlink_metadata(dangling)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn preserves_linked_managed_directories() {
        let root = tempfile::tempdir().unwrap();
        let external = root.path().join("external");
        seed(&external.join("Builder"));
        let managed = root.path().join("managed");
        fs::create_dir_all(&managed).unwrap();
        super::super::create_directory_link(&external, &managed.join("classes")).unwrap();
        refresh_managed_roots(&managed, "Builder", "agent-1").unwrap();
        assert_eq!(
            fs::read_to_string(external.join("Builder/CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
    }

    #[test]
    fn ordinary_claude_bootstrap_refreshes_class_and_final_habitat_memory() {
        let root = tempfile::tempdir().unwrap();
        let managed = root.path().join("managed");
        let workspace = root.path().join("workspace");
        seed(&managed.join("classes/Builder"));
        seed(&workspace);
        fs::write(workspace.join("CLAUDE.md"), "@../untrusted.md\n").unwrap();
        let habitat = {
            let _guard = crate::utils::wardian_test_env_lock();
            let previous = std::env::var_os("WARDIAN_HOME");
            unsafe { std::env::set_var("WARDIAN_HOME", &managed) };
            let result = super::super::prepare_provider_habitat(
                "claude",
                &workspace,
                "Builder",
                Some("agent-1"),
            );
            match previous {
                Some(value) => unsafe { std::env::set_var("WARDIAN_HOME", value) },
                None => unsafe { std::env::remove_var("WARDIAN_HOME") },
            }
            result.unwrap().unwrap()
        };
        super::super::append_habitat_memory_instructions(
            &habitat,
            Some("memory acceptance marker"),
        )
        .unwrap();
        let class = fs::read_to_string(managed.join("classes/Builder/CLAUDE.md")).unwrap();
        assert!(class.starts_with(MARKER));
        assert!(class.ends_with("# Owned instructions\n"));
        let canonical = fs::read_to_string(habitat.join("AGENTS.md")).unwrap();
        let projected = fs::read_to_string(habitat.join("CLAUDE.md")).unwrap();
        assert!(projected.ends_with(&canonical));
        assert!(projected.contains("# Owned instructions"));
        assert!(projected.contains("memory acceptance marker"));
        assert_eq!(
            fs::read_to_string(workspace.join("CLAUDE.md")).unwrap(),
            "@../untrusted.md\n"
        );
    }
}
