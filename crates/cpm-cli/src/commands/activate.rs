//! `cpm activate` — persist the active group for `cpm sync`.

use clap::Args;
use cpm_core::{
    project::{load_manifest, write_manifest},
    CpmError,
};

use super::style_success;

/// Arguments for `cpm activate`.
#[derive(Debug, Args)]
pub struct ActivateArgs {
    /// Name of the group to activate.
    ///
    /// When provided, `cpm sync` will automatically use this group without
    /// requiring `--group` on every invocation.  Omit together with `--clear`
    /// to deactivate the current group.
    pub group: Option<String>,

    /// Clear the currently active group (revert to default-only installs).
    #[arg(long, conflicts_with = "group")]
    pub clear: bool,
}

pub async fn run(args: ActivateArgs) -> Result<(), CpmError> {
    let manifest_path = std::path::Path::new("cpm.toml");
    let mut manifest = load_manifest(manifest_path)?;

    if args.clear {
        manifest.settings.active_group = None;
        write_manifest(manifest_path, &manifest)?;
        println!("{} active group cleared", style_success("✓"));
        return Ok(());
    }

    match args.group {
        Some(group) => {
            manifest.settings.active_group = Some(group.clone());
            write_manifest(manifest_path, &manifest)?;
            println!("{} active group set to '{group}'", style_success("✓"));
        }
        None => {
            // No group and no --clear: just show the current active group.
            match manifest.settings.active_group.as_deref() {
                Some(group) => println!("active group: {group}"),
                None => println!("no active group set (using 'default')"),
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use cpm_types::{Manifest, PartialSettings};
    use indexmap::IndexMap;

    fn make_manifest_with_active_group(group: Option<&str>) -> Manifest {
        Manifest {
            settings: PartialSettings {
                active_group: group.map(ToOwned::to_owned),
                ..Default::default()
            },
            plugins: IndexMap::new(),
            skills: IndexMap::new(),
            agents: IndexMap::new(),
            mcps: IndexMap::new(),
            hooks: IndexMap::new(),
            workflows: IndexMap::new(),
            instructions: IndexMap::new(),
            groups: IndexMap::new(),
            package: None,
            sources: IndexMap::new(),
        }
    }

    #[test]
    fn active_group_round_trips_through_partial_settings() {
        let manifest = make_manifest_with_active_group(Some("dev"));
        assert_eq!(manifest.settings.active_group.as_deref(), Some("dev"));
    }

    #[test]
    fn no_active_group_is_empty() {
        let manifest = make_manifest_with_active_group(None);
        assert!(manifest.settings.active_group.is_none());
        assert!(manifest.settings.is_empty());
    }

    #[test]
    fn active_group_makes_settings_non_empty() {
        let manifest = make_manifest_with_active_group(Some("research"));
        assert!(!manifest.settings.is_empty());
    }
}
