//! Migration helpers for retiring Wardian-generated provider instruction files.

use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Component, Path};

const CLAUDE_PROJECTION_MARKER: &str =
    "<!-- Wardian Claude projection v1; source=AGENTS.md; body_sha256=";
const LEGACY_PROVIDER_FILES: &[&str] = &["CLAUDE.md", "GEMINI.md"];

/// Remove legacy provider instruction files from a Wardian-owned instruction
/// directory when their contents still prove that Wardian generated them.
///
/// `boundary` is the trusted Wardian-owned tree root. Every directory from
/// that boundary through `root` must be a real directory. Custom files, links,
/// hardlinks, and unrecognized generated formats are preserved so migration
/// cannot delete user-authored instruction sources.
pub fn retire_legacy_provider_instruction_files(
    boundary: &Path,
    root: &Path,
) -> Result<(), String> {
    if !is_unlinked_managed_directory(boundary, root)? {
        return Ok(());
    }

    for filename in LEGACY_PROVIDER_FILES {
        retire_owned_file(&root.join(filename), filename)?;
    }
    Ok(())
}

/// Retire generated provider instruction files from every known Wardian
/// instruction root. Direct child and habitat directories are never followed
/// through links.
pub fn retire_legacy_provider_instruction_tree(home: &Path) -> Result<(), String> {
    retire_legacy_provider_instruction_files(home, &home.join("common"))?;
    retire_children(home, &home.join("classes"), false)?;
    retire_children(home, &home.join("agents"), true)
}

fn retire_children(boundary: &Path, parent: &Path, include_habitat: bool) -> Result<(), String> {
    if !is_unlinked_managed_directory(boundary, parent)? {
        return Ok(());
    }

    for entry in fs::read_dir(parent)
        .map_err(|error| format!("Could not read {}: {error}", parent.display()))?
    {
        let path = entry
            .map_err(|error| format!("Could not read {}: {error}", parent.display()))?
            .path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
        if !metadata.is_dir() || is_link(&metadata) {
            continue;
        }
        retire_legacy_provider_instruction_files(boundary, &path)?;
        if include_habitat {
            retire_legacy_provider_instruction_files(boundary, &path.join("habitat"))?;
        }
    }
    Ok(())
}

fn is_unlinked_managed_directory(boundary: &Path, root: &Path) -> Result<bool, String> {
    let Ok(relative) = root.strip_prefix(boundary) else {
        return Ok(false);
    };
    let mut current = boundary.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            if !matches!(component, Component::Normal(_)) {
                return Ok(false);
            }
            current.push(component);
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !is_link(&metadata) => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "Could not inspect managed path {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(true)
}

fn retire_owned_file(path: &Path, filename: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !is_link(&metadata) => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not inspect {}: {error}", path.display())),
    }
    if has_multiple_links(path)? {
        return Ok(());
    }

    let bytes =
        fs::read(path).map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let owned =
        is_legacy_stub(&bytes) || (filename == "CLAUDE.md" && is_owned_claude_projection(&bytes));
    if owned {
        fs::remove_file(path)
            .map_err(|error| format!("Could not remove {}: {error}", path.display()))?;
    }
    Ok(())
}

fn is_legacy_stub(bytes: &[u8]) -> bool {
    matches!(bytes, b"@AGENTS.md" | b"@AGENTS.md\n" | b"@AGENTS.md\r\n")
}

fn is_owned_claude_projection(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Some((header, body)) = text.split_once('\n') else {
        return false;
    };
    header
        == format!(
            "{CLAUDE_PROJECTION_MARKER}{:x} -->",
            Sha256::digest(body.as_bytes())
        )
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn has_multiple_links(path: &Path) -> Result<bool, String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

        let file = fs::File::open(path)
            .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: file owns a live handle, and the API initializes info on success.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr()) }
            == 0
        {
            return Err(format!(
                "Could not inspect links for {}: {}",
                path.display(),
                io::Error::last_os_error()
            ));
        }
        Ok(unsafe { info.assume_init() }.nNumberOfLinks > 1)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path)
            .map(|metadata| metadata.nlink() > 1)
            .map_err(|error| format!("Could not inspect links for {}: {error}", path.display()))
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

    fn projection(body: &str) -> String {
        format!(
            "{CLAUDE_PROJECTION_MARKER}{:x} -->\n{body}",
            Sha256::digest(body.as_bytes())
        )
    }

    #[test]
    fn retires_owned_stubs_and_claude_projection() {
        let home = tempfile::tempdir().expect("home");
        for root in [
            home.path().join("common"),
            home.path().join("classes/Builder"),
            home.path().join("agents/agent-1"),
            home.path().join("agents/agent-1/habitat"),
        ] {
            fs::create_dir_all(&root).expect("instruction root");
            fs::write(root.join("GEMINI.md"), "@AGENTS.md\n").expect("gemini stub");
            fs::write(root.join("CLAUDE.md"), projection("# Canonical\n"))
                .expect("claude projection");
        }

        retire_legacy_provider_instruction_tree(home.path()).expect("retire tree");
        retire_legacy_provider_instruction_tree(home.path()).expect("repeat retirement");

        for root in [
            home.path().join("common"),
            home.path().join("classes/Builder"),
            home.path().join("agents/agent-1"),
            home.path().join("agents/agent-1/habitat"),
        ] {
            assert!(!root.join("GEMINI.md").exists());
            assert!(!root.join("CLAUDE.md").exists());
        }
    }

    #[test]
    fn preserves_custom_mutated_and_hardlinked_files() {
        let root = tempfile::tempdir().expect("root");
        let claude = root.path().join("CLAUDE.md");
        let gemini = root.path().join("GEMINI.md");
        fs::write(&claude, "# Custom Claude\n").expect("custom claude");
        fs::write(&gemini, "@AGENTS.md\n").expect("gemini stub");
        fs::hard_link(&gemini, root.path().join("gemini-alias.md")).expect("hardlink");

        retire_legacy_provider_instruction_files(root.path(), root.path()).expect("retire files");

        assert_eq!(
            fs::read_to_string(claude).expect("claude"),
            "# Custom Claude\n"
        );
        assert_eq!(fs::read_to_string(gemini).expect("gemini"), "@AGENTS.md\n");

        fs::remove_file(root.path().join("gemini-alias.md")).expect("remove alias");
        fs::write(
            root.path().join("CLAUDE.md"),
            format!("{}edited\n", projection("body\n")),
        )
        .expect("mutated projection");
        retire_legacy_provider_instruction_files(root.path(), root.path()).expect("retire files");
        assert!(root.path().join("CLAUDE.md").exists());
        assert!(!root.path().join("GEMINI.md").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn preserves_linked_instruction_roots() {
        let home = tempfile::tempdir().expect("home");
        let external = tempfile::tempdir().expect("external");
        fs::write(external.path().join("CLAUDE.md"), "@AGENTS.md\n").expect("claude stub");
        fs::write(external.path().join("GEMINI.md"), "@AGENTS.md\n").expect("gemini stub");
        fs::create_dir_all(home.path().join("classes")).expect("classes");
        let linked = home.path().join("classes/Linked");

        #[cfg(unix)]
        std::os::unix::fs::symlink(external.path(), &linked).expect("linked class root");
        #[cfg(windows)]
        junction::create(external.path(), &linked).expect("linked class root");

        retire_legacy_provider_instruction_tree(home.path()).expect("retire tree");

        assert!(external.path().join("CLAUDE.md").exists());
        assert!(external.path().join("GEMINI.md").exists());

        #[cfg(windows)]
        junction::delete(&linked).expect("remove linked class root");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn preserves_instruction_roots_beneath_linked_ancestors() {
        let home = tempfile::tempdir().expect("home");
        let external_classes = tempfile::tempdir().expect("external classes");
        let external_agent = tempfile::tempdir().expect("external agent");
        let class = external_classes.path().join("Builder");
        let habitat = external_agent.path().join("habitat");
        fs::create_dir_all(&class).expect("class");
        fs::create_dir_all(&habitat).expect("habitat");
        for root in [&class, &habitat] {
            fs::write(root.join("CLAUDE.md"), "@AGENTS.md\n").expect("claude stub");
            fs::write(root.join("GEMINI.md"), "@AGENTS.md\n").expect("gemini stub");
        }
        fs::create_dir_all(home.path().join("agents")).expect("agents");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(external_classes.path(), home.path().join("classes"))
                .expect("linked classes parent");
            std::os::unix::fs::symlink(external_agent.path(), home.path().join("agents/agent-1"))
                .expect("linked agent parent");
        }
        #[cfg(windows)]
        {
            junction::create(external_classes.path(), home.path().join("classes"))
                .expect("linked classes parent");
            junction::create(external_agent.path(), home.path().join("agents/agent-1"))
                .expect("linked agent parent");
        }

        retire_legacy_provider_instruction_files(home.path(), &home.path().join("classes/Builder"))
            .expect("retire class files");
        retire_legacy_provider_instruction_files(
            home.path(),
            &home.path().join("agents/agent-1/habitat"),
        )
        .expect("retire habitat files");

        for root in [&class, &habitat] {
            assert!(root.join("CLAUDE.md").exists());
            assert!(root.join("GEMINI.md").exists());
        }

        #[cfg(windows)]
        {
            junction::delete(home.path().join("classes")).expect("remove classes junction");
            junction::delete(home.path().join("agents/agent-1")).expect("remove agent junction");
        }
    }
}
