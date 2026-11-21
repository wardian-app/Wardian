use super::*;
use serde_json::json;

#[test]
fn positive_value_reuse_preserves_typed_fields_and_charges_failed_reads() {
    #[derive(Deserialize)]
    struct OwnerOnly {
        owner: String,
    }
    #[derive(Debug, Deserialize, PartialEq)]
    struct WithOrder {
        owner: String,
        source_order: String,
    }
    let temp = tempfile::tempdir().unwrap();
    let writer = Store::writer(temp.path());
    let reference = writer
        .put(&json!({ "owner": "synthetic-owner", "source_order": "0001" }))
        .unwrap();
    let mut reader = Store::new(temp.path());
    let mut cache = SeekCache::default();
    let narrow: OwnerOnly = reader.read_reusing_values(&reference, &mut cache).unwrap();
    assert_eq!(narrow.owner, "synthetic-owner");
    let spent = (reader.objects, reader.bytes);
    let full: WithOrder = reader.read_reusing_values(&reference, &mut cache).unwrap();
    assert_eq!(
        full.source_order, "0001",
        "the first typed view must not discard fields"
    );
    assert_eq!((reader.objects, reader.bytes), spent);

    let invalid = writer.put(&json!({ "owner": false })).unwrap();
    assert!(reader
        .read_reusing_values::<OwnerOnly>(&invalid, &mut cache)
        .is_err());
    assert_eq!(reader.objects, spent.0 + 1);
    assert!(!cache.values.contains_key(&invalid));
    reader
        .read_reusing_values::<serde_json::Value>(&invalid, &mut cache)
        .unwrap();
    assert_eq!(reader.objects, spent.0 + 2);

    let bytes = fs::read(temp.path().join(&reference)).unwrap();
    fs::write(temp.path().join(&reference), b"changed").unwrap();
    let mut fresh = SeekCache::default();
    let before = (reader.objects, reader.bytes);
    assert!(reader
        .read_reusing_values::<WithOrder>(&reference, &mut fresh)
        .is_err());
    assert_eq!((reader.objects, reader.bytes), (before.0 + 1, before.1 + 7));
    assert!(fresh.values.is_empty());
    fs::write(temp.path().join(&reference), bytes).unwrap();
    reader
        .read_reusing_values::<WithOrder>(&reference, &mut fresh)
        .unwrap();
    assert_eq!(reader.objects, before.0 + 2);

    let missing = "f".repeat(64);
    let before = reader.objects;
    for _ in 0..2 {
        assert!(reader
            .read_reusing_values::<WithOrder>(&missing, &mut fresh)
            .is_err());
    }
    assert_eq!(reader.objects, before + 2);
    assert!(!fresh.values.contains_key(&missing));
}

#[test]
fn positive_values_and_nodes_share_the_existing_cache_caps() {
    let temp = tempfile::tempdir().unwrap();
    // The existing writer allowance lets this test reach the cache's separate
    // bounds without renewing a normal reader's hard physical-read allowance.
    let mut store = Store::writer(temp.path());
    let first = store.put(&"value-0").unwrap();
    let root = store.insert(&None, "key", &first).unwrap();
    let mut cache = SeekCache::default();
    assert_eq!(
        store
            .get_reusing_nodes(&Some(root.clone()), "key", &mut cache)
            .unwrap(),
        Some(first)
    );
    let mut last = String::new();
    for index in 0..READ_OBJECTS {
        let value = format!("value-{index}");
        last = store.put(&value).unwrap();
        assert_eq!(
            store
                .read_reusing_values::<String>(&last, &mut cache)
                .unwrap(),
            value
        );
    }
    assert_eq!(
        (cache.nodes.len(), cache.values.len()),
        (1, READ_OBJECTS - 1)
    );
    let spent = store.objects;
    store
        .read_reusing_values::<String>(&last, &mut cache)
        .unwrap();
    assert_eq!(
        store.objects,
        spent + 1,
        "overflowed values are not retained"
    );
    assert!(cache.bytes <= READ_BYTES);

    let mut byte_cache = SeekCache::default();
    store
        .get_reusing_nodes(&Some(root), "key", &mut byte_cache)
        .unwrap();
    for index in 0..=READ_BYTES / (OBJECT_BYTES - 128) {
        let value = format!("{index}:{}", "x".repeat(OBJECT_BYTES - 128));
        last = store.put(&value).unwrap();
        assert_eq!(
            store
                .read_reusing_values::<String>(&last, &mut byte_cache)
                .unwrap(),
            value
        );
        assert!(byte_cache.bytes <= READ_BYTES);
    }
    assert_eq!(byte_cache.nodes.len(), 1);
    assert!(
        byte_cache.values.len() < READ_OBJECTS - 1,
        "the byte cap binds first"
    );
    let retained = (byte_cache.values.len(), byte_cache.bytes);
    let spent = store.objects;
    store
        .read_reusing_values::<String>(&last, &mut byte_cache)
        .unwrap();
    assert_eq!(store.objects, spent + 1);
    assert_eq!((byte_cache.values.len(), byte_cache.bytes), retained);
}

#[test]
fn optional_metadata_allowance_keeps_failed_reads_charged() {
    let temp = tempfile::tempdir().unwrap();
    let writer = Store::writer(temp.path());
    let reference = writer.put(&"synthetic value").unwrap();
    let mut reader = Store::new(temp.path());
    let limits = (reader.object_limit, reader.byte_limit);
    let result = reader.read_allowance(1, READ_BYTES, |reader| {
        reader.read::<String>(&reference)?;
        reader.read::<String>(&reference)
    });
    assert!(result.is_err());
    assert_eq!(reader.objects, 1);
    assert_eq!((reader.object_limit, reader.byte_limit), limits);
    reader.read::<String>(&reference).unwrap();
    assert_eq!(reader.objects, 2);
    let bytes = reader.bytes;
    assert!(reader
        .read_allowance(1, 0, |reader| reader.read::<String>(&reference))
        .is_err());
    assert_eq!(reader.objects, 3);
    assert_eq!(reader.bytes, bytes);
    assert_eq!((reader.object_limit, reader.byte_limit), limits);
}

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
