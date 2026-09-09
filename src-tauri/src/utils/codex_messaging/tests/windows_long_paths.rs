//! Inert filesystem coverage for managed MCP publication before home compaction.
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
fn atomic_replace_supports_plain_and_verbatim_long_paths() {
    let temp = tempfile::tempdir().unwrap();
    for label in ["ascii", "unicode-\u{e9}\u{4e2d}"] {
        for length in [241, 280] {
            let parent = plain_parent(temp.path(), label, length);
            // 241 reproduces config=253 / ownership=265; 280 also puts
            // the temporary file itself beyond the legacy path boundary.
            assert_eq!(units(&parent.join("config.toml")), length + 12);
            assert_eq!(units(&parent.join(RECORD)), length + 24);
            for directory in [parent.clone(), parent.canonicalize().unwrap()] {
                for name in ["config.toml", RECORD] {
                    let path = directory.join(name);
                    for bytes in [b"initial bytes".as_slice(), b"replacement bytes"] {
                        atomic_replace(&path, bytes).unwrap_or_else(|error| {
                            panic!("{label}, parent={length}, path={path:?}: {error}")
                        });
                        assert_eq!(std::fs::read(&path).unwrap(), bytes);
                    }
                    std::fs::remove_file(&path).unwrap();
                }
            }
            assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        }
    }
}

#[test]
fn long_path_registration_keeps_ownership_and_preserves_unowned_config() {
    let temp = tempfile::tempdir().unwrap();
    let suffix = Path::new(r"agents\agent\habitat\.codex");
    for length in [241, 280] {
        let home = plain_parent(
            temp.path(),
            "unicode-\u{e9}\u{4e2d}",
            length - units(suffix) - 1,
        );
        let codex = prepare(&home, "agent", true);
        assert_eq!(units(&codex), length);
        assert_eq!(
            ensure_managed_messaging(&home, "agent").unwrap(),
            Registration::Updated
        );
        let config = std::fs::read(codex.join("config.toml")).unwrap();
        let record = std::fs::read(codex.join(RECORD)).unwrap();
        let owner: Ownership = serde_json::from_slice(&record).unwrap();
        assert_eq!(owner.agent_id, "agent");
        assert_eq!(owner.wardian_home, home.to_str().unwrap());
        assert!(owns(&read_config(&codex)["mcp_servers"][SERVER], &owner));
        assert_eq!(
            ensure_managed_messaging(&home, "agent").unwrap(),
            Registration::Unchanged
        );
        assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), config);
        assert_eq!(std::fs::read(codex.join(RECORD)).unwrap(), record);
        // Simulate interruption after config publication: do not adopt the
        // now-unowned entry or rewrite it when registration is retried.
        std::fs::remove_file(codex.join(RECORD)).unwrap();
        assert!(matches!(
            ensure_managed_messaging(&home, "agent").unwrap(),
            Registration::Unavailable(_)
        ));
        assert_eq!(std::fs::read(codex.join("config.toml")).unwrap(), config);
        assert!(!codex.join(RECORD).exists());
    }
}
