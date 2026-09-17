//! Exercise Codex launch-file publication on plain and verbatim long paths.
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
fn launch_publication_supports_plain_and_verbatim_long_paths() {
    let temp = tempfile::tempdir().unwrap();
    for label in ["ascii", "unicode-\u{e9}\u{4e2d}"] {
        for length in [241, 280] {
            let parent = plain_parent(temp.path(), label, length);
            assert!(units(&parent.join("config.toml")) > 250);
            assert!(units(&parent.join(journal::FILE)) > 260);
            let canonical_parent = parent.canonicalize().unwrap();
            std::fs::write(canonical_parent.join("config.toml"), "model = 'baseline'\n").unwrap();
            for home in [parent.clone(), parent.canonicalize().unwrap()] {
                let mut guard = prepare_launch_config(&home, &args(&["model='long-path'"]))
                    .unwrap_or_else(|error| {
                        panic!("{label}, parent={length}, home={home:?}: {error}")
                    });
                assert_eq!(read(&home)["model"].as_str(), Some("long-path"));
                assert!(home.join(journal::FILE).is_file());
                guard.restore().unwrap();
                // This regression covers long-path publication and journal cleanup;
                // TOML rendering may normalize quote style during restore.
                assert_eq!(read(&home)["model"].as_str(), Some("baseline"));
                assert!(!home.join(journal::FILE).exists());
            }
            assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
        }
    }
}
