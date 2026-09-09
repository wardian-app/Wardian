//! Filesystem security fixtures; these do not establish provider acceptance.
use super::{
    create_private_directory, create_private_root, root_candidates, validate_private_root,
};

#[test]
fn roots_and_slots_are_private_and_slot_creation_is_exclusive() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("root");
    create_private_root(&root).unwrap();
    let slot = root.join("01234567");
    create_private_directory(&slot).unwrap();
    validate_private_root(&slot).unwrap();
    std::fs::write(slot.join("marker"), b"retained").unwrap();
    assert_eq!(
        create_private_directory(&slot).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    create_private_root(&root).unwrap();
    assert_eq!(std::fs::read(slot.join("marker")).unwrap(), b"retained");
}

#[test]
fn existing_file_is_never_adopted_or_replaced() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("file");
    std::fs::write(&path, b"retained").unwrap();
    assert!(create_private_root(&path).is_err());
    assert!(validate_private_root(&path).is_err());
    assert_eq!(
        create_private_directory(&path).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"retained");
}

#[test]
fn final_directory_link_is_rejected_without_changing_target() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("target");
    let link = fixture.path().join("link");
    create_private_root(&target).unwrap();
    std::fs::write(target.join("marker"), b"retained").unwrap();
    // Existing helper uses unprivileged junctions on Windows, symlinks on Unix.
    crate::utils::fs::create_directory_link(&target, &link).unwrap();
    assert!(validate_private_root(&link).is_err());
    assert!(validate_private_root(&link.join("")).is_err());
    assert!(create_private_root(&link).is_err());
    assert!(create_private_directory(&link).is_err());
    assert_eq!(std::fs::read(target.join("marker")).unwrap(), b"retained");
    validate_private_root(&target).unwrap();
}

#[test]
fn creation_does_not_create_missing_ancestors_or_accept_relative_paths() {
    let fixture = tempfile::tempdir().unwrap();
    let parent = fixture.path().join("missing");
    assert_eq!(
        create_private_directory(&parent.join("slot"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(!parent.exists());
    assert_eq!(
        create_private_directory(std::path::Path::new("relative-slot"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert!(create_private_root(&fixture.path().join("..").join("escape")).is_err());
}

#[test]
fn discovery_preserves_wardian_priority_without_creating_candidates() {
    let fixture = tempfile::tempdir().unwrap();
    let roots = root_candidates(fixture.path()).unwrap();
    assert_eq!(roots.first().unwrap(), &fixture.path().join("c"));
    assert!(!roots[0].exists());
    assert!(roots.iter().all(|path| path.is_absolute()));
    let distinct: std::collections::HashSet<_> = roots.iter().collect();
    assert_eq!(distinct.len(), roots.len());
}

#[test]
fn missing_or_invalid_fallback_does_not_discard_valid_roots() {
    let fixture = tempfile::tempdir().unwrap();
    let primary = fixture.path().join("c");
    assert_eq!(
        super::ordered_candidates(fixture.path(), Vec::new()).unwrap(),
        vec![primary.clone()]
    );
    let valid = fixture.path().join("fallback");
    let roots = super::ordered_candidates(
        fixture.path(),
        vec![
            std::path::PathBuf::from("relative"),
            fixture.path().join("..").join("invalid"),
            valid.clone(),
            primary.clone(),
        ],
    )
    .unwrap();
    assert_eq!(roots, vec![primary, valid]);
}

#[cfg(unix)]
#[test]
fn broad_unix_permissions_are_rejected_without_chmod() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("hostile");
    create_private_directory(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(validate_private_root(&root).is_err());
    assert!(create_private_root(&root).is_err());
    assert_eq!(std::fs::metadata(&root).unwrap().mode() & 0o777, 0o755);
}

#[cfg(windows)]
#[test]
fn protected_everyone_dacl_is_rejected_without_repair() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("hostile");
    // Default owner is this user; a protected DACL alone is insufficient.
    super::native::create_with_descriptor(&root, "D:P(A;OICI;FA;;;WD)").unwrap();
    std::fs::write(root.join("marker"), b"retained").unwrap();
    assert!(validate_private_root(&root).is_err());
    assert!(create_private_root(&root).is_err());
    assert!(validate_private_root(&root).is_err());
    assert_eq!(std::fs::read(root.join("marker")).unwrap(), b"retained");
}

#[cfg(windows)]
#[test]
fn inherited_user_only_dacl_is_rejected_without_repair() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("root");
    create_private_directory(&root).unwrap();
    let inherited = root.join("inherited");
    std::fs::create_dir(&inherited).unwrap();
    assert!(validate_private_root(&inherited).is_err());
    assert!(create_private_root(&inherited).is_err());
    assert!(validate_private_root(&inherited).is_err());
}

#[cfg(windows)]
#[test]
fn long_unicode_parent_supports_private_creation_and_validation() {
    use std::os::windows::ffi::OsStrExt;
    let fixture = tempfile::tempdir().unwrap();
    let mut parent = fixture.path().to_path_buf();
    for _ in 0..5 {
        parent.push("長".repeat(54));
    }
    std::fs::create_dir_all(&parent).unwrap();
    let root = parent.join("private");
    assert!(root.as_os_str().encode_wide().count() > 260);
    create_private_directory(&root).unwrap();
    validate_private_root(&root).unwrap();
    create_private_root(&root).unwrap();
    assert_eq!(
        create_private_directory(&root).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    std::fs::write(root.join("marker"), b"retained").unwrap();
    assert_eq!(std::fs::read(root.join("marker")).unwrap(), b"retained");
}
