use super::FileResourceErrorV1;
use crate::models::AgentConfig;
use std::fs;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

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

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("directory symlink");
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) {
        junction::create(target, link).expect("directory junction");
    }
}
