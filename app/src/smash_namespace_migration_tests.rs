use super::*;

#[test]
fn migration_moves_legacy_smash_data_once() {
    let temp_dir = tempfile::tempdir().unwrap();
    let old_dir = temp_dir.path().join(".warp-oss");
    let new_dir = temp_dir.path().join(".smash");
    std::fs::create_dir(&old_dir).unwrap();
    std::fs::write(old_dir.join("settings.toml"), "theme = 'dark'").unwrap();

    migrate_directory(&old_dir, &new_dir);

    assert!(!old_dir.exists());
    assert_eq!(
        std::fs::read_to_string(new_dir.join("settings.toml")).unwrap(),
        "theme = 'dark'"
    );

    migrate_directory(&old_dir, &new_dir);
    assert!(new_dir.exists());
}
