use super::*;

#[test]
fn materializes_all_assets_and_preserves_unrelated_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = materialize(dir.path()).unwrap();
    fs::write(root.join("user-note.txt"), "keep").unwrap();
    fs::write(root.join("plugins/smash/scripts/notify.sh"), "old").unwrap();
    assert_eq!(materialize(dir.path()).unwrap(), root);
    for (relative_path, contents) in FILES {
        assert_eq!(fs::read(root.join(relative_path)).unwrap(), *contents);
    }
    assert_eq!(
        fs::read_to_string(root.join("user-note.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn manifest_matches_installer_and_hook_paths_exist() {
    let dir = tempfile::tempdir().unwrap();
    let root = materialize(dir.path()).unwrap().join("plugins/smash");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".codex-plugin/plugin.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], super::super::PLUGIN_NAME);
    assert_eq!(manifest["version"], super::super::MINIMUM_PLUGIN_VERSION);
    assert_eq!(manifest["interface"]["displayName"], "Smash");
    let hooks: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("hooks/hooks.json")).unwrap()).unwrap();
    assert_eq!(hooks["hooks"].as_object().unwrap().len(), 5);
    assert!(root.join("scripts/notify.sh").is_file());
}
