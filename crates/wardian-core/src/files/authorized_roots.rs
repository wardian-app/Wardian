use super::FileResourceErrorV1;
use crate::models::AgentConfig;
use std::fs;
use std::path::{Path, PathBuf};

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
        let canonical_path = canonicalize(requested, "requested file")?;
        if !fs::metadata(&canonical_path)
            .map_err(|error| unavailable("requested file", error))?
            .is_file()
        {
            return Err(FileResourceErrorV1::new(
                "unavailable_path",
                format!("requested path is not a file: {}", requested.display()),
            ));
        }

        self.roots
            .iter()
            .find(|root| component_contains(root, &canonical_path))
            .cloned()
            .map(|root| AuthorizedPath {
                canonical_path,
                root,
            })
            .ok_or_else(|| {
                FileResourceErrorV1::new(
                    "unauthorized_path",
                    format!(
                        "requested file is outside the agent's authorized roots: {}",
                        requested.display()
                    ),
                )
            })
    }
}

/// A canonical file path paired with the canonical root that authorized it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedPath {
    /// The fully resolved file path, with links and junctions eliminated.
    pub canonical_path: PathBuf,
    /// The canonical configured root that contains `canonical_path`.
    pub root: PathBuf,
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
