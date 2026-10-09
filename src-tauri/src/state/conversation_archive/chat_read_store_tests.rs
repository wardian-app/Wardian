use super::*;
use serde_json::json;

fn insert_rows(store: &mut Store, count: usize) -> String {
    let mut root = None;
    for index in 0..count {
        let value = store.put(&format!("row-{index}")).unwrap();
        root = Some(
            store
                .insert(&root, &format!("{index:020}"), &value)
                .unwrap(),
        );
    }
    root.unwrap()
}

fn file_count(directory: &Path) -> usize {
    fs::read_dir(directory)
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_file())
        .count()
}

#[test]
fn checkpoint_keeps_the_same_root_and_flushes_fewer_objects() {
    let temp = tempfile::tempdir().unwrap();
    let immediate_path = temp.path().join("immediate");
    let checkpoint_path = temp.path().join("checkpoint");
    let mut immediate = Store::writer(&immediate_path);
    let original_root = insert_rows(&mut immediate, 12);
    let mut checkpoint = Store::checkpoint_writer(&checkpoint_path);
    let root = insert_rows(&mut checkpoint, 12);
    assert_eq!(root, original_root, "published format/tree shape changed");
    assert!(!checkpoint_path.join(&root).exists());
    assert!(Store::new(&checkpoint_path)
        .get(&Some(root.clone()), "00000000000000000011")
        .is_err());
    assert!(checkpoint
        .get(&Some(root.clone()), "00000000000000000011")
        .unwrap()
        .is_some());
    checkpoint
        .finish_checkpoint(&[Dependency::Node(root.clone())])
        .unwrap();
    let expected = Store::new(&immediate_path)
        .page(&Some(original_root), None, 12)
        .unwrap();
    let actual = Store::new(&checkpoint_path)
        .page(&Some(root), None, 12)
        .unwrap();
    assert_eq!(actual, expected);
    assert!(file_count(&checkpoint_path) < file_count(&immediate_path));
    assert!(checkpoint.pending.as_ref().unwrap().nodes.is_empty());
}

#[test]
fn checkpoint_follows_typed_body_links_and_ignores_hash_shaped_user_data() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::checkpoint_writer(temp.path());
    let body = store.put_bytes(b"body payload").unwrap();
    let body_root = store.insert(&None, "00000000000000000000", &body).unwrap();
    let orphan_value = store.put(&"orphan").unwrap();
    let orphan = store.insert(&None, "unused", &orphan_value).unwrap();
    let row = store
        .put_linked(
            &json!({"body": {"root": body_root}, "text": orphan, "metadata": {"root": orphan}}),
            vec![Dependency::Node(body_root.clone())],
        )
        .unwrap();
    // Both a key and arbitrary payload text look like a pending node reference.
    let root = store.insert(&None, &orphan, &row).unwrap();
    store
        .finish_checkpoint(&[Dependency::Node(root.clone())])
        .unwrap();
    assert!(!temp.path().join(&orphan).exists());
    assert!(temp.path().join(&body_root).is_file());
    let mut reader = Store::new(temp.path());
    assert_eq!(reader.get(&Some(root), &orphan).unwrap(), Some(row));
    assert_eq!(
        reader
            .get(&Some(body_root), "00000000000000000000")
            .unwrap(),
        Some(body)
    );
}

#[test]
fn checkpoint_retains_body_roots_referenced_only_by_change_objects() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::checkpoint_writer(temp.path());
    let body = store.put_bytes(b"changed body").unwrap();
    let root = store.insert(&None, "offset", &body).unwrap();
    let row = store
        .put_linked(
            &json!({"body": {"root": root}}),
            vec![Dependency::Node(root.clone())],
        )
        .unwrap();
    store.finish_checkpoint(&[Dependency::Object(row)]).unwrap();
    assert!(temp.path().join(root).is_file());
}

#[test]
fn checkpoint_keeps_children_when_a_body_blob_matches_a_new_node() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::checkpoint_writer(temp.path());
    let value = store.put_bytes(b"value").unwrap();
    let child = store.insert(&None, "child", &value).unwrap();
    let node = Node {
        key: "parent".into(),
        value: value.clone(),
        left: Some(child.clone()),
        right: None,
        height: 2,
    };
    let blob = store
        .put_bytes(&serde_json::to_vec(&node).unwrap())
        .unwrap();
    let root = store.save(node).unwrap();
    assert_eq!(blob, root, "same bytes share the same object namespace");
    store
        .finish_checkpoint(&[Dependency::Node(root.clone())])
        .unwrap();
    assert!(temp.path().join(child).is_file());
    assert_eq!(
        Store::new(temp.path()).get(&Some(root), "child").unwrap(),
        Some(value)
    );
}

#[test]
fn failed_partial_flush_preserves_old_roots_and_retries_idempotently() {
    let temp = tempfile::tempdir().unwrap();
    let mut immediate = Store::writer(temp.path());
    let old_root = insert_rows(&mut immediate, 1);
    let old_bytes = fs::read(temp.path().join(&old_root)).unwrap();
    let mut checkpoint = Store::checkpoint_writer(temp.path());
    let root = insert_rows(&mut checkpoint, 3);
    let node: Node =
        serde_json::from_slice(&checkpoint.pending.as_ref().unwrap().nodes[&root].bytes).unwrap();
    let child = node.right.unwrap();
    // A real rename target collision fails after its earlier child was flushed.
    let blocked = temp.path().join(&root);
    fs::create_dir(&blocked).unwrap();
    assert!(checkpoint
        .finish_checkpoint(&[Dependency::Node(root.clone())])
        .is_err());
    assert!(temp.path().join(&child).is_file());
    assert_eq!(fs::read(temp.path().join(&old_root)).unwrap(), old_bytes);
    drop(checkpoint);
    fs::remove_dir(&blocked).unwrap();
    let mut retry = Store::checkpoint_writer(temp.path());
    let retried_root = insert_rows(&mut retry, 3);
    assert_eq!(retried_root, root);
    retry.finish_checkpoint(&[Dependency::Node(root)]).unwrap();
    assert_eq!(fs::read(temp.path().join(old_root)).unwrap(), old_bytes);
}

#[test]
fn checkpoint_ram_cap_fails_before_private_nodes_are_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::checkpoint_writer(temp.path());
    let value = store.put(&"value").unwrap();
    store.pending.as_mut().unwrap().ram_limit = 1;
    let error = store.insert(&None, "key", &value).unwrap_err();
    assert!(error.to_string().contains("RAM budget"));
    assert!(store.pending.as_ref().unwrap().nodes.is_empty());
    assert_eq!(file_count(temp.path()), 1);
}

#[test]
fn checkpoint_counts_duplicate_write_attempts_without_recharging_flush() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::checkpoint_writer(temp.path());
    let value = store.put(&"value").unwrap();
    let root = store.insert(&None, "key", &value).unwrap();
    let attempts = store.write_objects.get();
    let attempted_bytes = store.write_bytes.get();
    store.finish_checkpoint(&[Dependency::Node(root)]).unwrap();
    assert_eq!(store.write_objects.get(), attempts);
    assert_eq!(store.write_bytes.get(), attempted_bytes);
    store.object_limit = attempts + 1;
    assert_eq!(store.put(&"value").unwrap(), value);
    assert!(store.put(&"value").is_err());
}

#[test]
fn immediate_writers_still_persist_nodes_without_a_checkpoint_flush() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::bounded_writer(temp.path());
    let value = store.put(&"value").unwrap();
    let root = store.insert(&None, "key", &value).unwrap();
    assert!(store.pending.is_none());
    assert!(temp.path().join(&root).is_file());
    assert_eq!(
        Store::new(temp.path()).get(&Some(root), "key").unwrap(),
        Some(value)
    );
}
