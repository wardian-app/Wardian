use std::fs;
use std::io;
use std::path::Path;

/// Create a directory link (junction on Windows, symlink on Unix) without
/// spawning any external process. Creates the link's parent directories.
pub fn create_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(windows)]
    {
        junction::create(target, link)
    }

    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link)
    }
}

/// Remove whatever occupies `path`: a reparse point / symlink is unlinked
/// without following it; a real directory or file is removed recursively.
pub fn remove_existing_deployment(path: &Path) -> io::Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };

    if metadata.file_type().is_symlink() {
        #[cfg(windows)]
        {
            return fs::remove_dir(path).or_else(|_| fs::remove_file(path));
        }
        #[cfg(not(windows))]
        {
            return fs::remove_file(path);
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return fs::remove_dir(path).or_else(|_| fs::remove_file(path));
        }
    }

    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let destination = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

/// Replace any existing deployment at `dst_dir` with a link to `src_dir`,
/// falling back to a copy when linking fails. Returns `true` when copied.
pub fn deploy_skill_dir(src_dir: &Path, dst_dir: &Path) -> io::Result<bool> {
    deploy_skill_dir_with_linker(src_dir, dst_dir, create_directory_link)
}

pub(crate) fn deploy_skill_dir_with_linker<F>(
    src_dir: &Path,
    dst_dir: &Path,
    linker: F,
) -> io::Result<bool>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    remove_existing_deployment(dst_dir)?;
    if let Some(parent) = dst_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match linker(src_dir, dst_dir) {
        Ok(()) => Ok(false),
        Err(_) => {
            copy_dir_all(src_dir, dst_dir)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn link_is_live_and_removal_preserves_target() {
        let temp = tempfile::tempdir().expect("temp dir");
        let target = temp.path().join("source-skill");
        let link = temp.path().join("deploys").join("planner");
        fs::create_dir_all(&target).expect("target dir");
        fs::write(target.join("SKILL.md"), "one").expect("skill file");

        create_directory_link(&target, &link).expect("create link");
        fs::write(target.join("SKILL.md"), "two").expect("update source");
        assert_eq!(fs::read_to_string(link.join("SKILL.md")).expect("read via link"), "two");

        remove_existing_deployment(&link).expect("remove link");
        assert!(!link.exists());
        assert!(target.join("SKILL.md").exists(), "removing link must not touch target");
    }

    #[test]
    fn removing_parent_dir_of_link_preserves_target() {
        let temp = tempfile::tempdir().expect("temp dir");
        let target = temp.path().join("source-skill");
        let parent = temp.path().join("class-skills");
        fs::create_dir_all(&target).expect("target dir");
        fs::write(target.join("SKILL.md"), "keep me").expect("skill file");
        create_directory_link(&target, &parent.join("planner")).expect("link");

        fs::remove_dir_all(&parent).expect("remove parent");
        assert_eq!(fs::read_to_string(target.join("SKILL.md")).expect("target intact"), "keep me");
    }

    #[test]
    fn deploy_skill_dir_falls_back_to_copy() {
        let temp = tempfile::tempdir().expect("temp dir");
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");
        fs::create_dir_all(src.join("nested")).expect("src dirs");
        fs::write(src.join("SKILL.md"), "s").expect("skill");
        fs::write(src.join("nested").join("n.md"), "n").expect("nested");

        let copied = deploy_skill_dir_with_linker(&src, &dst, |_, _| {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"))
        })
        .expect("fallback");
        assert!(copied);
        assert_eq!(fs::read_to_string(dst.join("nested").join("n.md")).expect("copied"), "n");
    }

    #[test]
    fn deploy_skill_dir_replaces_existing_deployment() {
        let temp = tempfile::tempdir().expect("temp dir");
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");
        fs::create_dir_all(&src).expect("src");
        fs::write(src.join("SKILL.md"), "new").expect("skill");
        fs::create_dir_all(&dst).expect("stale dst");
        fs::write(dst.join("SKILL.md"), "stale").expect("stale file");

        let copied = deploy_skill_dir(&src, &dst).expect("deploy");
        assert!(!copied);
        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).expect("read"), "new");
    }
}
