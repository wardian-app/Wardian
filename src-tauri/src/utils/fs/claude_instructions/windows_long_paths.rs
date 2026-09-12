//! Exercise the actual Claude projection writer on Windows long-path aliases.
use super::*;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

fn utf16_units(path: &Path) -> usize {
    path.as_os_str().encode_wide().count()
}

fn plain_path(path: &Path) -> PathBuf {
    let text = path.to_str().expect("test path is valid Unicode");
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

fn long_root(unicode: bool, verbatim: bool) -> (tempfile::TempDir, PathBuf) {
    let fixture = tempfile::Builder::new()
        .prefix("wardian-claude-long-path-")
        .tempdir()
        .expect("create short fixture root");
    let mut plain = plain_path(fixture.path());
    let component = if unicode {
        "é中😀-segment0123456789"
    } else {
        "ascii-segment0123456789"
    };
    while utf16_units(&plain) < 280 {
        plain.push(component);
    }
    fs::create_dir_all(&plain).expect("create actual long directory before writer call");
    assert!(plain.is_absolute());
    assert!(!plain.to_string_lossy().starts_with(r"\\?\"));
    assert!(utf16_units(&plain) >= 280);
    let canonical = fs::canonicalize(&plain).expect("resolve existing parent alias");
    assert!(canonical.to_string_lossy().starts_with(r"\\?\"));
    assert!(same_file::is_same_file(&plain, &canonical).unwrap());
    (fixture, if verbatim { canonical } else { plain })
}

fn seed_owned(root: &Path, body: &str) {
    let mut projected = format!("{MARKER}{:x} -->\n", Sha256::digest(body.as_bytes()));
    projected.push_str(body);
    fs::write(root.join("CLAUDE.md"), projected).expect("seed a valid owned snapshot");
}

fn assert_projection(root: &Path, body: &str) {
    let source = root.join("AGENTS.md");
    let target = root.join("CLAUDE.md");
    assert!(utf16_units(&plain_path(&target)) >= 260);
    let projection = fs::read(&target).expect("read actual published projection");
    assert!(is_owned(&projection));
    assert!(projection.ends_with(body.as_bytes()));
    assert_eq!(fs::read(&source).unwrap(), body.as_bytes());
    let alias = fs::canonicalize(&target).unwrap();
    assert!(same_file::is_same_file(&target, &alias).unwrap());
    assert_eq!(fs::read(alias).unwrap(), projection);
    let mut children = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    children.sort();
    assert_eq!(
        children,
        vec![
            std::ffi::OsString::from("AGENTS.md"),
            std::ffi::OsString::from("CLAUDE.md")
        ]
    );
}

fn habitat_case(unicode: bool, verbatim: bool, owned_refresh: bool) {
    let (_fixture, root) = long_root(unicode, verbatim);
    let body = "# Current managed instructions\nUnicode β 😀\n@../untouched.md\n";
    fs::write(root.join("AGENTS.md"), body).expect("seed readable canonical source");
    if owned_refresh {
        seed_owned(&root, "# Previous owned snapshot\n");
        assert!(is_owned(&fs::read(root.join("CLAUDE.md")).unwrap()));
    } else {
        assert!(!root.join("CLAUDE.md").exists());
    }
    refresh_habitat(&root).unwrap_or_else(|error| panic!(
        "actual refresh_habitat failed (unicode={unicode}, verbatim={verbatim}, owned_refresh={owned_refresh}, parent_utf16={}): {error}",
        utf16_units(&root)
    ));
    assert_projection(&root, body);
    let projected = fs::read(root.join("CLAUDE.md")).unwrap();
    refresh_habitat(&root).expect("unchanged owned projection is idempotent");
    assert_eq!(fs::read(root.join("CLAUDE.md")).unwrap(), projected);
}

fn managed_case(unicode: bool, verbatim: bool) {
    let (_fixture, root) = long_root(unicode, verbatim);
    let roots = [
        root.join("common"),
        root.join("classes/Builder"),
        root.join("agents/agent-one"),
    ];
    for managed in &roots {
        fs::create_dir_all(managed).unwrap();
        fs::write(managed.join("AGENTS.md"), "# Refreshed managed root\n").unwrap();
        seed_owned(managed, "# Previous owned root\n");
    }
    refresh_managed_roots(&root, "Builder", "agent-one").unwrap_or_else(|error| panic!(
        "actual refresh_managed_roots failed (unicode={unicode}, verbatim={verbatim}, parent_utf16={}): {error}",
        utf16_units(&root)
    ));
    for managed in roots {
        assert_projection(&managed, "# Refreshed managed root\n");
    }
}

#[test]
fn ascii_plain_habitat_creation() {
    habitat_case(false, false, false);
}

#[test]
fn ascii_plain_habitat_owned_refresh() {
    habitat_case(false, false, true);
}

#[test]
fn ascii_plain_managed_owned_refresh() {
    managed_case(false, false);
}

#[test]
fn unicode_plain_habitat_creation() {
    habitat_case(true, false, false);
}

#[test]
fn unicode_plain_habitat_owned_refresh() {
    habitat_case(true, false, true);
}

#[test]
fn unicode_plain_managed_owned_refresh() {
    managed_case(true, false);
}

#[test]
fn ascii_verbatim_habitat_creation() {
    habitat_case(false, true, false);
}

#[test]
fn ascii_verbatim_habitat_owned_refresh() {
    habitat_case(false, true, true);
}

#[test]
fn ascii_verbatim_managed_owned_refresh() {
    managed_case(false, true);
}

#[test]
fn unicode_verbatim_habitat_creation() {
    habitat_case(true, true, false);
}

#[test]
fn unicode_verbatim_habitat_owned_refresh() {
    habitat_case(true, true, true);
}

#[test]
fn unicode_verbatim_managed_owned_refresh() {
    managed_case(true, true);
}
