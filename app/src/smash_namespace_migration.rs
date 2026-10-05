use std::path::Path;

use warp_core::channel::{Channel, ChannelState};

pub(crate) fn migrate_smash_home_if_needed() {
    if ChannelState::channel() != Channel::Oss {
        return;
    }

    let Some(home_dir) = dirs::home_dir() else {
        return;
    };
    let old_dir = home_dir.join(".warp-oss");
    let Some(new_dir) = warp_core::paths::smash_home_config_dir() else {
        return;
    };

    migrate_directory(&old_dir, &new_dir);
}

fn migrate_directory(old_dir: &Path, new_dir: &Path) {
    if new_dir.exists() || !old_dir.exists() {
        return;
    }

    if let Err(err) = std::fs::rename(old_dir, new_dir) {
        log::warn!(
            "Failed to migrate Smash data from {} to {}: {err}",
            old_dir.display(),
            new_dir.display()
        );
    } else {
        log::info!(
            "Migrated Smash data from {} to {}",
            old_dir.display(),
            new_dir.display()
        );
    }
}

#[cfg(test)]
#[path = "smash_namespace_migration_tests.rs"]
mod tests;
