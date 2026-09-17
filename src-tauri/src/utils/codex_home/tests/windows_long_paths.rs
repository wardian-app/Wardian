//! Exercise Codex ownership publication on plain and verbatim long paths.
use super::*;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

fn units(path: &Path) -> usize {
    path.as_os_str().encode_wide().count()
}

fn plain_parent(root: &Path, label: &str, length: usize) -> PathBuf {
    let canonical = root.canonicalize().unwrap();
    let spelling = canonical.to_str().unwrap();
    let plain = if let Some(unc) = spelling.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        spelling.strip_prefix(r"\\?\").unwrap().to_owned()
    };
    let mut parent = PathBuf::from(plain).join(label);
    assert!(parent.is_absolute());
    assert!(units(&parent) + 2 < length, "fixture root is too long");
    while units(&parent) + 91 < length {
        parent.push("p".repeat(90));
    }
    parent.push("p".repeat(length - units(&parent) - 1));
    std::fs::create_dir_all(&parent).unwrap();
    assert_eq!(units(&parent), length);
    parent
}

#[test]
fn ownership_publication_supports_plain_and_verbatim_long_paths() {
    let temp = tempfile::tempdir().unwrap();
    for label in ["ascii", "unicode-\u{e9}\u{4e2d}"] {
        for length in [241, 280] {
            let parent = plain_parent(temp.path(), label, length);
            assert!(units(&parent.join(RECORD)) > 260);
            for directory in [parent.clone(), parent.canonicalize().unwrap()] {
                let path = directory.join(RECORD);
                let record = serde_json::json!({
                    "version": 1,
                    "marker": format!("{label}-{length}"),
                });
                storage::publish_new(&path, &record).unwrap_or_else(|error| {
                    panic!("{label}, parent={length}, path={path:?}: {error}")
                });
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap())
                        .unwrap(),
                    record
                );
                let error = storage::publish_new(&path, &record).unwrap_err();
                assert!(error.contains("already exists"));
                std::fs::remove_file(path).unwrap();
            }
            assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        }
    }
}
