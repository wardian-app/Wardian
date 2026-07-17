use super::FileResourceErrorV1;
use crate::models::AgentConfig;
use std::fs;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use sha2::{Digest, Sha256};

#[cfg(windows)]
use std::path::Component;

/// Canonical filesystem roots that an agent was explicitly granted access to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedRootService {
    roots: Vec<PathBuf>,
}

impl AuthorizedRootService {
    /// Builds the agent's file-publication boundary from its workspace and
    /// user-provided include directories. System includes are intentionally not
    /// publication roots because they contain Wardian-managed context.
    ///
    /// # Errors
    ///
    /// Returns `unavailable_path` if any configured publication root is empty,
    /// missing, unreadable, or not a directory.
    pub fn from_agent_config(config: &AgentConfig) -> Result<Self, FileResourceErrorV1> {
        let configured_roots = std::iter::once(config.folder.as_str()).chain(
            config
                .include_directories
                .iter()
                .flatten()
                .map(String::as_str),
        );

        let mut roots = Vec::new();
        for configured_root in configured_roots {
            if configured_root.trim().is_empty() {
                return Err(FileResourceErrorV1::new(
                    "unavailable_path",
                    "an authorized root is empty",
                ));
            }
            let canonical_root = canonicalize(Path::new(configured_root), "authorized root")?;
            if !fs::metadata(&canonical_root)
                .map_err(|error| unavailable("authorized root", error))?
                .is_dir()
            {
                return Err(FileResourceErrorV1::new(
                    "unavailable_path",
                    format!("authorized root is not a directory: {configured_root}"),
                ));
            }
            if !roots.iter().any(|root| root == &canonical_root) {
                roots.push(canonical_root);
            }
        }

        Ok(Self { roots })
    }

    /// Returns the canonical workspace and user include roots.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Resolves a requested existing file and proves that its canonical path is
    /// component-contained by one of the canonical configured roots.
    ///
    /// # Errors
    ///
    /// Returns `unavailable_path` when the target cannot resolve to an existing
    /// file, or `unauthorized_path` when its canonical location is outside all
    /// configured roots.
    pub fn authorize_existing_file(
        &self,
        requested: &Path,
    ) -> Result<AuthorizedPath, FileResourceErrorV1> {
        let requested_path = absolute_path(requested)?;
        let canonical_path = canonicalize(&requested_path, "requested file")?;
        let root = self
            .roots
            .iter()
            .find(|root| component_contains(root, &canonical_path))
            .cloned()
            .ok_or_else(|| unauthorized(requested))?;
        let file =
            File::open(&canonical_path).map_err(|error| unavailable("requested file", error))?;
        let metadata = file
            .metadata()
            .map_err(|error| unavailable("requested file", error))?;
        if !metadata.is_file() {
            return Err(FileResourceErrorV1::new(
                "unavailable_path",
                format!("requested path is not a file: {}", requested.display()),
            ));
        }
        let identity = FileIdentity::from_file(&file)
            .map_err(|error| unavailable("requested file identity", error))?;
        verify_binding(&requested_path, &canonical_path, &root, identity)?;

        Ok(AuthorizedPath {
            canonical_path: canonical_path.clone(),
            root: root.clone(),
            requested_path,
            verified_canonical_path: canonical_path,
            verified_root: root,
            identity,
            file: Arc::new(Mutex::new(file)),
        })
    }
}

/// A canonical file path paired with the canonical root and open file identity
/// that authorized it.
#[derive(Debug, Clone)]
pub struct AuthorizedPath {
    /// The fully resolved file path, with links and junctions eliminated.
    pub canonical_path: PathBuf,
    /// The canonical configured root that contains `canonical_path`.
    pub root: PathBuf,
    requested_path: PathBuf,
    verified_canonical_path: PathBuf,
    verified_root: PathBuf,
    identity: FileIdentity,
    file: Arc<Mutex<File>>,
}

impl AuthorizedPath {
    /// Returns the exact pathname whose provenance was authorized. Unlike
    /// [`Self::canonical_path`], this retains an alias, symlink, or junction
    /// spelling so callers can revalidate that specific access path.
    #[must_use]
    pub fn requested_path(&self) -> &Path {
        &self.requested_path
    }

    /// Reopens the originally requested pathname after an editor-style atomic
    /// replacement while preserving the authorization boundary that approved
    /// it.
    ///
    /// A replacement is accepted only when the original pathname still
    /// canonicalizes to the exact same target under the exact same canonical
    /// root. A symlink or junction retarget, including one that remains inside
    /// the root, is rejected rather than silently changing the resource's
    /// identity.
    ///
    /// # Errors
    ///
    /// Returns `unauthorized_path` when the pathname no longer resolves to the
    /// previously approved canonical target or escapes its root, and
    /// `unavailable_path` when the replacement cannot be opened as a file.
    pub fn reauthorize_same_target(&self) -> Result<Self, FileResourceErrorV1> {
        let canonical_path = fs::canonicalize(&self.requested_path)
            .map(normalize_drive_letter)
            .map_err(|_| unauthorized(&self.requested_path))?;
        if canonical_path != self.verified_canonical_path
            || !component_contains(&self.verified_root, &canonical_path)
        {
            return Err(unauthorized(&self.requested_path));
        }

        let file =
            File::open(&canonical_path).map_err(|error| unavailable("replacement file", error))?;
        let metadata = file
            .metadata()
            .map_err(|error| unavailable("replacement file", error))?;
        if !metadata.is_file() {
            return Err(FileResourceErrorV1::new(
                "unavailable_path",
                format!(
                    "replacement path is not a file: {}",
                    self.requested_path.display()
                ),
            ));
        }
        let identity = FileIdentity::from_file(&file)
            .map_err(|error| unavailable("replacement file identity", error))?;
        verify_binding(
            &self.requested_path,
            &self.verified_canonical_path,
            &self.verified_root,
            identity,
        )?;

        Ok(Self {
            canonical_path: self.canonical_path.clone(),
            root: self.root.clone(),
            requested_path: self.requested_path.clone(),
            verified_canonical_path: self.verified_canonical_path.clone(),
            verified_root: self.verified_root.clone(),
            identity,
            file: Arc::new(Mutex::new(file)),
        })
    }

    /// Reads validated UTF-8 text from the retained authorized file handle for
    /// one exact verified revision.
    ///
    /// `maximum_length_bytes` is an additional caller-selected ceiling. It can
    /// only narrow the renderer ceiling sealed into `revision`.
    ///
    /// # Errors
    ///
    /// Returns `unauthorized_path` if the original pathname no longer binds to
    /// the retained file, `stale_revision` if size or SHA-256 differs,
    /// `file_too_large` if the content exceeds either ceiling, and
    /// `unsupported_content` if the verified bytes are not UTF-8.
    pub fn read_verified_text(
        &self,
        revision: &FileRevisionToken,
        maximum_length_bytes: u64,
    ) -> Result<String, FileResourceErrorV1> {
        let bytes = self.read_verified_bytes(revision, None, maximum_length_bytes, true)?;
        String::from_utf8(bytes)
            .map_err(|_| FileResourceErrorV1::new("unsupported_content", "file is not valid UTF-8"))
    }

    /// Reads one inclusive byte range from the retained authorized file handle
    /// for one exact verified revision.
    ///
    /// The entire bounded file is hashed while only the requested range is
    /// captured. `maximum_length_bytes` limits the returned allocation and can
    /// never increase the renderer ceiling sealed into `revision`.
    ///
    /// # Errors
    ///
    /// Returns `unauthorized_path` if the original pathname no longer binds to
    /// the retained file, `stale_revision` if size or SHA-256 differs,
    /// `file_too_large` if the file or requested allocation exceeds its limit,
    /// and a range-specific error for an invalid interval.
    pub fn read_verified_range(
        &self,
        revision: &FileRevisionToken,
        range: RangeInclusive<u64>,
        maximum_length_bytes: u64,
    ) -> Result<Vec<u8>, FileResourceErrorV1> {
        let (start, end) = range.into_inner();
        self.read_verified_bytes(revision, Some((start, end)), maximum_length_bytes, true)
    }

    /// Verifies that the retained authorized handle still contains the exact
    /// revision without allocating or returning content bytes.
    ///
    /// # Errors
    ///
    /// Returns `unauthorized_path` if the original pathname binding changed,
    /// `stale_revision` if size or SHA-256 differs, and `file_too_large` if the
    /// content exceeds the renderer ceiling sealed into `revision`.
    pub fn verify_revision(&self, revision: &FileRevisionToken) -> Result<(), FileResourceErrorV1> {
        self.read_verified_bytes(revision, None, 0, false).map(drop)
    }

    /// Streams one exact verified revision into a caller-owned destination.
    ///
    /// This performs the same retained-handle identity, size, and SHA-256
    /// verification as the bounded read methods, but never materializes the
    /// complete file in memory. It is intended for immutable renderer-ticket
    /// snapshots that serve repeated ranges without rehashing the source.
    ///
    /// # Errors
    ///
    /// Returns the same authorization, revision, and size errors as
    /// [`Self::verify_revision`], or `runtime_unavailable` if the destination
    /// cannot accept the complete verified revision.
    pub fn copy_verified_revision_to(
        &self,
        revision: &FileRevisionToken,
        destination: &mut impl Write,
    ) -> Result<u64, FileResourceErrorV1> {
        self.scan_verified_revision(revision, |_, bytes| {
            destination.write_all(bytes).map_err(|error| {
                FileResourceErrorV1::new(
                    "runtime_unavailable",
                    format!("cannot write immutable file snapshot: {error}"),
                )
            })
        })
    }

    fn read_verified_bytes(
        &self,
        revision: &FileRevisionToken,
        range: Option<(u64, u64)>,
        maximum_length_bytes: u64,
        capture_bytes: bool,
    ) -> Result<Vec<u8>, FileResourceErrorV1> {
        let expected_size_bytes = revision.size_bytes;

        let selected_range = if !capture_bytes {
            None
        } else {
            match range {
                Some((start, end)) if start <= end && end < expected_size_bytes => {
                    Some((start, end))
                }
                Some((start, end)) if start > end => {
                    return Err(FileResourceErrorV1::new(
                        "invalid_range",
                        "byte range start exceeds its end",
                    ));
                }
                Some(_) => {
                    return Err(FileResourceErrorV1::new(
                        "range_not_satisfiable",
                        "byte range is outside the expected file revision",
                    ));
                }
                None if expected_size_bytes == 0 => None,
                None => Some((0, expected_size_bytes - 1)),
            }
        };
        let selected_len = selected_range
            .map(|(start, end)| end - start + 1)
            .unwrap_or_default();
        if selected_len > maximum_length_bytes {
            return Err(FileResourceErrorV1::new(
                "file_too_large",
                "selected content exceeds the allowed read length",
            ));
        }
        let selected_capacity: usize = selected_len.try_into().map_err(|_| {
            FileResourceErrorV1::new(
                "file_too_large",
                "selected byte range cannot fit in process memory",
            )
        })?;
        let mut selected = Vec::with_capacity(selected_capacity);

        self.scan_verified_revision(revision, |offset, bytes| {
            if let Some((start, end)) = selected_range {
                let read_u64 = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                let next_offset = offset.saturating_add(read_u64);
                let chunk_end = next_offset.saturating_sub(1);
                if offset <= end && chunk_end >= start {
                    let copy_start = start.saturating_sub(offset) as usize;
                    let copy_end = (end.min(chunk_end) - offset + 1) as usize;
                    selected.extend_from_slice(&bytes[copy_start..copy_end]);
                }
            }
            Ok(())
        })?;
        Ok(selected)
    }

    fn scan_verified_revision(
        &self,
        revision: &FileRevisionToken,
        mut consume: impl FnMut(u64, &[u8]) -> Result<(), FileResourceErrorV1>,
    ) -> Result<u64, FileResourceErrorV1> {
        if revision.identity != self.identity
            || revision.canonical_path != self.verified_canonical_path
        {
            return Err(FileResourceErrorV1::new(
                "unauthorized_path",
                "file revision token belongs to another authorized file",
            ));
        }
        let expected_hash = match &revision.fingerprint {
            FileRevisionFingerprint::ExactSha256(hash) => hash,
            FileRevisionFingerprint::BoundedSha256(_) => {
                return Err(FileResourceErrorV1::new(
                    "file_too_large",
                    "bounded metadata revisions cannot authorize content reads",
                ));
            }
        };
        let expected_size_bytes = revision.size_bytes;
        let maximum_size_bytes = revision.maximum_size_bytes;
        if expected_size_bytes > maximum_size_bytes {
            return Err(FileResourceErrorV1::new(
                "file_too_large",
                "file revision exceeds the allowed read size",
            ));
        }

        let mut file = self.lock_verified_file()?;
        let metadata = file.metadata().map_err(|error| {
            FileResourceErrorV1::new(
                "unavailable_path",
                format!("cannot read authorized file metadata: {error}"),
            )
        })?;
        if metadata.len() > maximum_size_bytes {
            return Err(FileResourceErrorV1::new(
                "file_too_large",
                "current file exceeds the allowed read size",
            ));
        }
        if metadata.len() != expected_size_bytes {
            return Err(FileResourceErrorV1::new(
                "stale_revision",
                "file size no longer matches the expected revision",
            ));
        }

        file.seek(SeekFrom::Start(0)).map_err(|error| {
            FileResourceErrorV1::new(
                "unavailable_path",
                format!("cannot seek authorized file: {error}"),
            )
        })?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut offset = 0_u64;
        loop {
            let read = file.read(&mut buffer).map_err(|error| {
                FileResourceErrorV1::new(
                    "unavailable_path",
                    format!("cannot read authorized file: {error}"),
                )
            })?;
            if read == 0 {
                break;
            }
            let read_u64 = u64::try_from(read).unwrap_or(u64::MAX);
            let next_offset = offset.saturating_add(read_u64);
            if next_offset > maximum_size_bytes {
                return Err(FileResourceErrorV1::new(
                    "file_too_large",
                    "file grew beyond the allowed read size",
                ));
            }
            hasher.update(&buffer[..read]);
            consume(offset, &buffer[..read])?;
            offset = next_offset;
        }

        self.verify_current_binding(&file)?;
        if offset != expected_size_bytes {
            return Err(FileResourceErrorV1::new(
                "stale_revision",
                "file length changed while reading the expected revision",
            ));
        }
        let actual_hash = format!("sha256:{:x}", hasher.finalize());
        if actual_hash != expected_hash.as_str() {
            return Err(FileResourceErrorV1::new(
                "stale_revision",
                "file content no longer matches the expected revision",
            ));
        }
        Ok(offset)
    }

    pub(super) fn verified_canonical_path(&self) -> &Path {
        &self.verified_canonical_path
    }

    pub(super) fn lock_verified_file(&self) -> Result<MutexGuard<'_, File>, FileResourceErrorV1> {
        let file = self.file.lock().map_err(|_| {
            FileResourceErrorV1::new("unavailable_path", "authorized file handle is unavailable")
        })?;
        self.verify_current_binding(&file)?;
        Ok(file)
    }

    pub(super) fn verify_current_binding(&self, file: &File) -> Result<(), FileResourceErrorV1> {
        let metadata = file.metadata().map_err(|error| {
            FileResourceErrorV1::new(
                "unavailable_path",
                format!("cannot read authorized file metadata: {error}"),
            )
        })?;
        let current_identity = FileIdentity::from_file(file).map_err(|error| {
            FileResourceErrorV1::new(
                "unavailable_path",
                format!("cannot read authorized file identity: {error}"),
            )
        })?;
        if !metadata.is_file() || current_identity != self.identity {
            return Err(FileResourceErrorV1::new(
                "unauthorized_path",
                "authorized file handle no longer identifies the approved file",
            ));
        }
        verify_binding(
            &self.requested_path,
            &self.verified_canonical_path,
            &self.verified_root,
            self.identity,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    volume: u64,
    file: u64,
}

impl FileIdentity {
    pub(super) fn fingerprint_components(self) -> (u64, u64) {
        (self.volume, self.file)
    }
}

/// Opaque capability proving that a descriptor was scanned from one retained
/// authorized file handle at one bounded content revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRevisionToken {
    identity: FileIdentity,
    canonical_path: PathBuf,
    fingerprint: FileRevisionFingerprint,
    size_bytes: u64,
    maximum_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FileRevisionFingerprint {
    ExactSha256(String),
    BoundedSha256(String),
}

impl FileRevisionFingerprint {
    pub(super) fn descriptor_value(&self) -> &str {
        match self {
            Self::ExactSha256(value) | Self::BoundedSha256(value) => value,
        }
    }
}

impl FileRevisionToken {
    pub(super) fn new(
        identity: FileIdentity,
        canonical_path: PathBuf,
        fingerprint: FileRevisionFingerprint,
        size_bytes: u64,
        maximum_size_bytes: u64,
    ) -> Self {
        Self {
            identity,
            canonical_path,
            fingerprint,
            size_bytes,
            maximum_size_bytes,
        }
    }
}

#[cfg(unix)]
impl FileIdentity {
    pub(super) fn from_file(file: &File) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;

        let metadata = file.metadata()?;
        Ok(Self {
            volume: metadata.dev(),
            file: metadata.ino(),
        })
    }
}

#[cfg(windows)]
impl FileIdentity {
    pub(super) fn from_file(file: &File) -> io::Result<Self> {
        use std::ffi::c_void;
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle;

        #[repr(C)]
        #[allow(non_snake_case)]
        struct FileTime {
            dwLowDateTime: u32,
            dwHighDateTime: u32,
        }

        #[repr(C)]
        #[allow(non_snake_case)]
        struct ByHandleFileInformation {
            dwFileAttributes: u32,
            ftCreationTime: FileTime,
            ftLastAccessTime: FileTime,
            ftLastWriteTime: FileTime,
            dwVolumeSerialNumber: u32,
            nFileSizeHigh: u32,
            nFileSizeLow: u32,
            nNumberOfLinks: u32,
            nFileIndexHigh: u32,
            nFileIndexLow: u32,
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                file: *mut c_void,
                information: *mut ByHandleFileInformation,
            ) -> i32;
        }

        let mut information = MaybeUninit::<ByHandleFileInformation>::uninit();
        // SAFETY: `file` is an open OS handle and Windows initializes the full
        // output structure when the call reports success.
        let succeeded =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful call above initialized `information`.
        let information = unsafe { information.assume_init() };
        Ok(Self {
            volume: u64::from(information.dwVolumeSerialNumber),
            file: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        })
    }
}

#[cfg(not(any(unix, windows)))]
impl FileIdentity {
    pub(super) fn from_file(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            volume: 0,
            file: metadata.len(),
        })
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, FileResourceErrorV1> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| unavailable("current directory", error))
}

fn verify_binding(
    requested_path: &Path,
    expected_canonical_path: &Path,
    root: &Path,
    expected_identity: FileIdentity,
) -> Result<(), FileResourceErrorV1> {
    let current_canonical_path = fs::canonicalize(requested_path)
        .map(normalize_drive_letter)
        .map_err(|_| unauthorized(requested_path))?;
    if current_canonical_path != expected_canonical_path
        || !component_contains(root, &current_canonical_path)
    {
        return Err(unauthorized(requested_path));
    }
    let current_file =
        File::open(&current_canonical_path).map_err(|_| unauthorized(requested_path))?;
    let current_metadata = current_file
        .metadata()
        .map_err(|_| unauthorized(requested_path))?;
    let current_identity =
        FileIdentity::from_file(&current_file).map_err(|_| unauthorized(requested_path))?;
    if !current_metadata.is_file() || current_identity != expected_identity {
        return Err(unauthorized(requested_path));
    }
    Ok(())
}

fn unauthorized(path: &Path) -> FileResourceErrorV1 {
    FileResourceErrorV1::new(
        "unauthorized_path",
        format!(
            "requested file is outside the agent's authorized roots or changed identity: {}",
            path.display()
        ),
    )
}

fn canonicalize(path: &Path, label: &str) -> Result<PathBuf, FileResourceErrorV1> {
    fs::canonicalize(path)
        .map(normalize_drive_letter)
        .map_err(|error| unavailable(label, error))
}

fn unavailable(label: &str, error: std::io::Error) -> FileResourceErrorV1 {
    FileResourceErrorV1::new(
        "unavailable_path",
        format!("cannot resolve {label}: {error}"),
    )
}

fn component_contains(root: &Path, candidate: &Path) -> bool {
    let mut root_components = root.components();
    let mut candidate_components = candidate.components();

    root_components.all(|root_component| candidate_components.next() == Some(root_component))
}

#[cfg(not(windows))]
fn normalize_drive_letter(path: PathBuf) -> PathBuf {
    path
}

#[cfg(windows)]
fn normalize_drive_letter(path: PathBuf) -> PathBuf {
    use std::path::Prefix;

    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return path;
    };
    let normalized_prefix = match prefix.kind() {
        Prefix::Disk(letter) => Some(format!("{}:", (letter as char).to_ascii_uppercase())),
        Prefix::VerbatimDisk(letter) => {
            Some(format!(r"\\?\{}:", (letter as char).to_ascii_uppercase()))
        }
        _ => None,
    };
    let Some(normalized_prefix) = normalized_prefix else {
        return path;
    };

    let mut normalized = PathBuf::from(normalized_prefix);
    for component in path.components().skip(1) {
        normalized.push(component.as_os_str());
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentConfig;
    use std::fs;
    use std::path::Path;

    fn config_with_roots(
        workspace: &Path,
        includes: &[&Path],
        system_includes: &[&Path],
    ) -> AgentConfig {
        AgentConfig {
            folder: workspace.to_string_lossy().into_owned(),
            include_directories: Some(
                includes
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            ),
            system_include_directories: Some(
                system_includes
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            ),
            ..AgentConfig::default()
        }
    }

    #[test]
    fn agent_roots_include_workspace_and_additional_directories_but_exclude_system_includes() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        let shared = temp.path().join("shared");
        let managed_skills = temp.path().join("managed-skills");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&shared).expect("shared");
        fs::create_dir_all(&managed_skills).expect("system include");
        fs::write(workspace.join("report.md"), "report").expect("workspace file");
        fs::write(shared.join("figure.png"), "figure").expect("shared file");
        fs::write(managed_skills.join("secret.md"), "secret").expect("system file");

        let config = config_with_roots(&workspace, &[&shared], &[&managed_skills]);
        let service = AuthorizedRootService::from_agent_config(&config).expect("valid roots");

        assert_eq!(service.roots().len(), 2);
        assert!(service
            .authorize_existing_file(&workspace.join("report.md"))
            .is_ok());
        assert!(service
            .authorize_existing_file(&shared.join("figure.png"))
            .is_ok());
        assert_eq!(
            service
                .authorize_existing_file(&managed_skills.join("secret.md"))
                .expect_err("system include must be rejected")
                .code(),
            "unauthorized_path",
        );
    }

    #[test]
    fn canonical_containment_rejects_parent_traversal() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.txt"), "secret").expect("outside file");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");

        let traversing_path = workspace.join("..").join("outside").join("secret.txt");
        assert_eq!(
            service
                .authorize_existing_file(&traversing_path)
                .expect_err("traversal must be rejected")
                .code(),
            "unauthorized_path",
        );
    }

    #[test]
    fn canonical_containment_rejects_string_prefix_sibling() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        let prefix_sibling = temp.path().join("workspace-secret");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&prefix_sibling).expect("prefix sibling");
        fs::write(prefix_sibling.join("secret.txt"), "secret").expect("sibling file");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");

        assert_eq!(
            service
                .authorize_existing_file(&prefix_sibling.join("secret.txt"))
                .expect_err("string-prefix sibling must be rejected")
                .code(),
            "unauthorized_path",
        );
    }

    #[test]
    fn canonical_containment_rejects_link_that_escapes_root() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.txt"), "secret").expect("outside file");
        create_directory_link(&outside, &workspace.join("linked-outside"));
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");

        assert_eq!(
            service
                .authorize_existing_file(&workspace.join("linked-outside").join("secret.txt"))
                .expect_err("linked escape must be rejected")
                .code(),
            "unauthorized_path",
        );
    }

    #[test]
    fn missing_target_and_unresolvable_root_fail_closed() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");

        assert_eq!(
            service
                .authorize_existing_file(&workspace.join("missing.txt"))
                .expect_err("missing file must fail")
                .code(),
            "unavailable_path",
        );

        let missing_root = temp.path().join("missing-root");
        assert_eq!(
            AuthorizedRootService::from_agent_config(&config_with_roots(&missing_root, &[], &[],))
                .expect_err("unresolvable root must fail")
                .code(),
            "unavailable_path",
        );
    }

    #[test]
    fn verified_snapshot_token_reads_text_and_ranges_from_the_retained_handle() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let path = workspace.join("payload.txt");
        fs::write(&path, b"0123456789").expect("fixture");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized file");
        let snapshot = super::super::VerifiedFileSnapshot::from_authorized_path(
            &authorized,
            &super::super::FileResourceLimits::default(),
        )
        .expect("verified snapshot");
        let revision = snapshot.revision_token();

        assert_eq!(
            authorized
                .read_verified_text(revision, 10)
                .expect("text read"),
            "0123456789"
        );
        assert_eq!(
            authorized
                .read_verified_range(revision, 2..=5, 4)
                .expect("range read"),
            b"2345"
        );
        authorized
            .verify_revision(revision)
            .expect("verification-only read");
    }

    #[test]
    fn retained_handle_read_methods_enforce_caller_length_limits() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let path = workspace.join("payload.txt");
        fs::write(&path, b"0123456789").expect("fixture");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized file");
        let snapshot = super::super::VerifiedFileSnapshot::from_authorized_path(
            &authorized,
            &super::super::FileResourceLimits::default(),
        )
        .expect("verified snapshot");

        assert_eq!(
            authorized
                .read_verified_text(snapshot.revision_token(), 9)
                .expect_err("text limit must be enforced")
                .code(),
            "file_too_large"
        );
        assert_eq!(
            authorized
                .read_verified_range(snapshot.revision_token(), 2..=5, 3)
                .expect_err("range limit must be enforced")
                .code(),
            "file_too_large"
        );
        assert_eq!(
            authorized
                .read_verified_range(snapshot.revision_token(), 8..=10, 3)
                .expect_err("range must remain within the snapshot")
                .code(),
            "range_not_satisfiable"
        );
    }

    #[test]
    fn verified_snapshot_token_rejects_stale_or_different_authorizations() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let path = workspace.join("payload.txt");
        fs::write(&path, b"0123456789").expect("fixture");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized file");
        let snapshot = super::super::VerifiedFileSnapshot::from_authorized_path(
            &authorized,
            &super::super::FileResourceLimits::default(),
        )
        .expect("verified snapshot");
        let revision = snapshot.revision_token();

        fs::write(&path, b"abcdefghij").expect("mutate same file");
        assert_eq!(
            authorized
                .verify_revision(revision)
                .expect_err("verification must reject the stale revision")
                .code(),
            "stale_revision"
        );
        assert_eq!(
            authorized
                .read_verified_text(revision, 10)
                .expect_err("old revision must fail")
                .code(),
            "stale_revision"
        );

        let other_path = workspace.join("other.txt");
        fs::write(&other_path, b"abcdefghij").expect("other fixture");
        let other_authorized = service
            .authorize_existing_file(&other_path)
            .expect("other authorized file");
        assert_eq!(
            other_authorized
                .read_verified_range(revision, 0..=3, 4)
                .expect_err("token must be bound to one authorized file")
                .code(),
            "unauthorized_path"
        );
    }

    #[test]
    fn same_path_atomic_replacement_can_be_reauthorized_without_changing_target() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let path = workspace.join("payload.txt");
        fs::write(&path, b"revision one").expect("fixture");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized file");

        let replacement = workspace.join("payload.replacement");
        fs::write(&replacement, b"revision two").expect("replacement fixture");
        replace_path_identity(&replacement, &path);

        let replacement = authorized
            .reauthorize_same_target()
            .expect("same target replacement must be authorized");
        let snapshot = super::super::VerifiedFileSnapshot::from_authorized_path(
            &replacement,
            &super::super::FileResourceLimits::default(),
        )
        .expect("replacement snapshot");
        assert_eq!(
            replacement
                .read_verified_text(snapshot.revision_token(), 12)
                .expect("replacement text"),
            "revision two"
        );
    }

    #[test]
    fn replacement_reauthorization_rejects_link_retarget_and_escape() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        let approved = workspace.join("approved");
        let other_inside = workspace.join("other-inside");
        let outside = temp.path().join("outside");
        for directory in [&workspace, &approved, &other_inside, &outside] {
            fs::create_dir_all(directory).expect("fixture directory");
            fs::write(
                directory.join("payload.txt"),
                directory.to_string_lossy().as_bytes(),
            )
            .expect("fixture file");
        }
        let link = workspace.join("current");
        create_directory_link(&approved, &link);
        let requested = link.join("payload.txt");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");
        let authorized = service
            .authorize_existing_file(&requested)
            .expect("authorized linked file");

        remove_directory_link(&link);
        create_directory_link(&other_inside, &link);
        assert_eq!(
            authorized
                .reauthorize_same_target()
                .expect_err("same-root link retarget must not change resource identity")
                .code(),
            "unauthorized_path"
        );

        remove_directory_link(&link);
        create_directory_link(&outside, &link);
        assert_eq!(
            authorized
                .reauthorize_same_target()
                .expect_err("link escape must remain denied")
                .code(),
            "unauthorized_path"
        );
    }

    #[test]
    fn verified_revision_can_be_streamed_once_into_an_immutable_destination() {
        let temp = tempfile::tempdir().expect("temp root");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let path = workspace.join("payload.pdf");
        fs::write(&path, b"%PDF-1.7 immutable bytes").expect("fixture");
        let service =
            AuthorizedRootService::from_agent_config(&config_with_roots(&workspace, &[], &[]))
                .expect("valid root");
        let authorized = service
            .authorize_existing_file(&path)
            .expect("authorized file");
        let snapshot = super::super::VerifiedFileSnapshot::from_authorized_path(
            &authorized,
            &super::super::FileResourceLimits::default(),
        )
        .expect("verified snapshot");
        let mut copied = Vec::new();

        assert_eq!(
            authorized
                .copy_verified_revision_to(snapshot.revision_token(), &mut copied)
                .expect("stream verified revision"),
            copied.len() as u64
        );
        assert_eq!(copied, b"%PDF-1.7 immutable bytes");
    }

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("directory symlink");
    }

    #[cfg(unix)]
    fn replace_path_identity(replacement: &Path, target: &Path) {
        fs::rename(replacement, target).expect("atomic replacement");
    }

    #[cfg(unix)]
    fn remove_directory_link(link: &Path) {
        fs::remove_file(link).expect("remove directory symlink");
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) {
        junction::create(target, link).expect("directory junction");
    }

    #[cfg(windows)]
    fn replace_path_identity(replacement: &Path, target: &Path) {
        // Common Windows editor saves move the prior identity aside before
        // moving the staged file into the stable pathname. This reproduces the
        // same retained-handle identity transition without relying on
        // `MoveFileExW(REPLACE_EXISTING)`, which rejects an open destination.
        let prior = target.with_extension("prior");
        fs::rename(target, &prior).expect("move prior identity aside");
        fs::rename(replacement, target).expect("move replacement into target");
        fs::remove_file(prior).expect("remove prior identity");
    }

    #[cfg(windows)]
    fn remove_directory_link(link: &Path) {
        junction::delete(link).expect("delete directory junction");
        fs::remove_dir(link).expect("remove directory junction");
    }
}
