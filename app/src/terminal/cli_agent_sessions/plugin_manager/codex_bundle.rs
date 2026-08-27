use std::fs;
use std::io;
use std::path::{Path, PathBuf};

macro_rules! asset {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/codex-plugins/",
                $path
            ))
            .as_slice(),
        )
    };
}

const FILES: &[(&str, &[u8])] = &[
    asset!(".agents/plugins/marketplace.json"),
    asset!("plugins/smash/.codex-plugin/plugin.json"),
    asset!("plugins/smash/hooks/hooks.json"),
    asset!("plugins/smash/scripts/notify.sh"),
    asset!("plugins/smash/assets/smash.svg"),
];

pub(super) fn materialize(codex_dir: &Path) -> io::Result<PathBuf> {
    let root = codex_dir.join("smash-integration");
    for (relative_path, contents) in FILES {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, contents)?;
    }
    Ok(root)
}

#[cfg(test)]
#[path = "codex_bundle_tests.rs"]
mod tests;
