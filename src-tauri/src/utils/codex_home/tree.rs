//! Copy and fingerprint directory entries without traversing links. No cleanup
//! removes partial copies, backups, unknown files or durable provider state.
use super::storage;
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn entries(path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(storage::error)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage::error)?;
    entries.sort();
    Ok(entries)
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

pub(super) fn snapshot(root: &Path) -> Result<String, String> {
    storage::plain_directory(root)?;
    let mut hash = Sha256::new();
    fingerprint(root, root, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn fingerprint(root: &Path, path: &Path, hash: &mut Sha256) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(storage::error)?;
    field(
        hash,
        path.strip_prefix(root)
            .map_err(|error| error.to_string())?
            .as_os_str()
            .as_encoded_bytes(),
    );
    if crate::utils::fs::is_directory_link(&metadata) {
        field(hash, b"link");
        let destination = std::fs::read_link(path).map_err(storage::error)?;
        validate_relative_link(root, path, &destination)?;
        field(hash, destination.as_os_str().as_encoded_bytes());
    } else if metadata.is_dir() {
        field(hash, b"directory");
        for entry in entries(path)? {
            fingerprint(root, &entry, hash)?;
        }
    } else if metadata.is_file() {
        field(hash, b"file");
        hash.update(metadata.len().to_le_bytes());
        let mut input = open_file(path)?;
        let mut bytes = [0u8; 65_536];
        loop {
            let count = input.read(&mut bytes).map_err(storage::error)?;
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
    } else {
        return Err(format!(
            "Unsupported live/special Codex state entry; stop its writer before migration: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_relative_link(root: &Path, path: &Path, destination: &Path) -> Result<(), String> {
    if destination.is_absolute() {
        return Ok(());
    }
    let joined = path.parent().ok_or("Link has no parent")?.join(destination);
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            component => resolved.push(component.as_os_str()),
        }
    }
    if !resolved.starts_with(root) {
        return Err(format!("Relative Codex link escapes its home; convert it to an absolute link before relocation: {}", path.display()));
    }
    Ok(())
}

fn open_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(storage::error)?;
    let metadata = file.metadata().map_err(storage::error)?;
    if !metadata.is_file() || crate::utils::fs::is_directory_link(&metadata) {
        return Err("Codex state entry changed during migration".into());
    }
    Ok(file)
}

pub(super) fn copy(source: &Path, target: &Path) -> Result<(), String> {
    if storage::exists(target)? {
        return Err(
            "Compact staging entry already exists; no overwrite or automatic replay".into(),
        );
    }
    copy_preflight(source)?;
    copy_entry(source, target)
}

fn copy_preflight(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(storage::error)?;
    super::copy_metadata::validate(path)?;
    if metadata.is_dir() && !crate::utils::fs::is_directory_link(&metadata) {
        for entry in entries(path)? {
            copy_preflight(&entry)?;
        }
    }
    Ok(())
}

fn copy_entry(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source).map_err(storage::error)?;
    if crate::utils::fs::is_directory_link(&metadata) {
        copy_link(source, target, &metadata)?;
    } else if metadata.is_dir() {
        std::fs::create_dir(target).map_err(storage::error)?;
        for entry in entries(source)? {
            copy_entry(
                &entry,
                &target.join(entry.file_name().ok_or("Invalid directory entry")?),
            )?;
        }
        std::fs::set_permissions(target, metadata.permissions()).map_err(storage::error)?;
        #[cfg(unix)]
        File::open(target)
            .and_then(|file| file.sync_all())
            .map_err(storage::error)?;
    } else if metadata.is_file() {
        let mut input = open_file(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(storage::error)?;
        std::io::copy(&mut input, &mut output).map_err(storage::error)?;
        output.flush().map_err(storage::error)?;
        let mut times = std::fs::FileTimes::new();
        if let Ok(modified) = metadata.modified() {
            times = times.set_modified(modified);
        }
        if let Ok(accessed) = metadata.accessed() {
            times = times.set_accessed(accessed);
        }
        output.set_times(times).map_err(storage::error)?;
        output
            .set_permissions(metadata.permissions())
            .map_err(storage::error)?;
        output.sync_all().map_err(storage::error)?;
    } else {
        return Err(format!(
            "Cannot copy special Codex state entry; original retained: {}",
            source.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn copy_link(source: &Path, target: &Path, _metadata: &Metadata) -> Result<(), String> {
    std::os::unix::fs::symlink(std::fs::read_link(source).map_err(storage::error)?, target)
        .map_err(storage::error)
}

#[cfg(windows)]
fn copy_link(source: &Path, target: &Path, metadata: &Metadata) -> Result<(), String> {
    use std::os::windows::fs::{FileTypeExt, MetadataExt};
    let destination = std::fs::read_link(source).map_err(storage::error)?;
    if junction::exists(source).map_err(storage::error)? {
        junction::create(destination, target).map_err(storage::error)
    } else if metadata.file_type().is_symlink_dir() {
        std::os::windows::fs::symlink_dir(destination, target).map_err(storage::error)
    } else if metadata.file_type().is_symlink_file() {
        std::os::windows::fs::symlink_file(destination, target).map_err(storage::error)
    } else if metadata.file_attributes() & 0x10 != 0 {
        // Directory junction: recreate the entry, never enumerate its target.
        junction::create(destination, target).map_err(storage::error)
    } else {
        Err("Unsupported Codex reparse entry; original and partial copy retained".into())
    }
}
