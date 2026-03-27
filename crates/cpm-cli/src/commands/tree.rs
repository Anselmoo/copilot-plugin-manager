//! `cpm tree` — show the dependency tree.

use clap::Args;
use cpm_core::{
    paths::portable_path_string,
    project::{load_global_lockfile, load_lockfile},
    CpmError,
};
use cpm_types::{AssetKind, GlobalLockfile, Lockfile, ResolvedAsset, SubAsset, SubAssetOwnership};
use serde::Serialize;
use std::collections::HashSet;

use super::{
    asset_install_target, asset_source_path, asset_source_url, display_groups,
    effective_asset_scope, format_sub_asset_summary, json_group, json_groups, json_rev,
    style_asset_heading, style_heading, style_label,
};

/// Arguments for `cpm tree`.
#[derive(Debug, Args)]
pub struct TreeArgs {
    /// Emit JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: TreeArgs) -> Result<(), CpmError> {
    let lockfile = load_lockfile(std::path::Path::new("cpm.lock"))?;
    let global_lockfile = load_global_lockfile()?;
    let visible_assets = collect_visible_assets(&lockfile, &global_lockfile);

    if args.json {
        let sections: Vec<_> = [
            AssetKind::Plugin,
            AssetKind::Skill,
            AssetKind::Agent,
            AssetKind::Mcp,
            AssetKind::Hook,
            AssetKind::Workflow,
            AssetKind::Instruction,
        ]
        .into_iter()
        .filter_map(|kind| {
            let mut assets: Vec<_> = visible_assets
                .iter()
                .filter(|entry| entry.asset.kind == kind)
                .collect();
            assets.sort_by(|left, right| left.asset.name.cmp(&right.asset.name));
            (!assets.is_empty()).then(|| TreeSection {
                kind: kind_header(kind).to_owned(),
                assets: assets
                    .into_iter()
                    .map(TreeAssetRow::from_entry)
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
        println!("{}", serde_json::to_string_pretty(&sections)?);
        return Ok(());
    }

    if visible_assets.is_empty() {
        println!("No visible assets found.");
        return Ok(());
    }

    let mut first_section = true;
    for kind in [
        AssetKind::Plugin,
        AssetKind::Skill,
        AssetKind::Agent,
        AssetKind::Mcp,
        AssetKind::Hook,
        AssetKind::Workflow,
        AssetKind::Instruction,
    ] {
        let mut assets: Vec<_> = visible_assets
            .iter()
            .filter(|entry| entry.asset.kind == kind)
            .collect();
        assets.sort_by(|left, right| left.asset.name.cmp(&right.asset.name));
        if assets.is_empty() {
            continue;
        }

        if !first_section {
            println!();
        }
        first_section = false;
        println!("{}", style_heading(kind_header(kind)));
        for entry in assets {
            let asset = &entry.asset;
            println!(
                "└── {}",
                style_asset_heading(asset.kind, effective_asset_scope(asset), &asset.name)
            );
            println!(
                "    {} {}",
                style_label("installed"),
                asset_install_target(asset)
            );
            if let Some(groups) = display_groups(&asset.source.groups) {
                println!("    {} {}", style_label("groups"), groups);
            }
            if let Some(claimed_by) = &entry.claimed_by {
                println!("    {} {}", style_label("claimed-by"), claimed_by);
            }
            if !asset.resolved_rev.is_empty() {
                println!("    {} {}", style_label("rev"), asset.resolved_rev);
            }
            if let Some(url) = asset_source_url(asset) {
                println!("    {} {url}", style_label("source-url"));
            }
            if let Some(path) = asset_source_path(asset) {
                println!("    {} {path}", style_label("source-path"));
            }
            if !asset.sub_assets.is_empty() {
                println!("    {}", style_label("sub-assets"));
                for (index, sub_asset) in
                    sorted_sub_assets(&asset.sub_assets).into_iter().enumerate()
                {
                    let branch = if index + 1 == asset.sub_assets.len() {
                        "└──"
                    } else {
                        "├──"
                    };
                    println!("        {branch} {}", format_sub_asset_summary(sub_asset));
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct TreeAssetEntry {
    asset: ResolvedAsset,
    claimed_by: Option<String>,
}

fn collect_visible_assets(
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
) -> Vec<TreeAssetEntry> {
    let mut seen = HashSet::new();
    let mut assets = Vec::new();

    for asset in lockfile.all_assets() {
        seen.insert(asset_identity_key(asset));
        assets.push(TreeAssetEntry {
            asset: asset.clone(),
            claimed_by: None,
        });
    }

    for claim in &global_lockfile.claims {
        let asset = &claim.asset;
        let key = asset_identity_key(asset);
        if seen.insert(key) {
            assets.push(TreeAssetEntry {
                asset: asset.clone(),
                claimed_by: Some(portable_path_string(claim.claimed_by.as_std_path())),
            });
        }
    }

    assets
}

fn asset_identity_key(asset: &ResolvedAsset) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        asset.kind,
        asset.name,
        effective_asset_scope(asset),
        asset.source.groups.join(","),
        asset.resolved_rev,
        asset.hash,
    )
}

#[derive(Debug, Serialize)]
struct TreeSection {
    kind: String,
    assets: Vec<TreeAssetRow>,
}

#[derive(Debug, Serialize)]
struct TreeAssetRow {
    name: String,
    kind: String,
    scope: String,
    group: Option<String>,
    groups: Option<Vec<String>>,
    claimed_by: Option<String>,
    rev: Option<String>,
    install_target: String,
    source_url: Option<String>,
    source_path: Option<String>,
    sub_assets: Vec<TreeSubAssetRow>,
}

impl TreeAssetRow {
    fn from_entry(entry: &TreeAssetEntry) -> Self {
        let asset = &entry.asset;
        Self {
            name: asset.name.clone(),
            kind: asset.kind.to_string(),
            scope: effective_asset_scope(asset).to_string(),
            group: json_group(&asset.source.groups),
            groups: json_groups(&asset.source.groups),
            claimed_by: entry.claimed_by.clone(),
            rev: json_rev(&asset.resolved_rev),
            install_target: asset_install_target(asset),
            source_url: asset_source_url(asset).map(ToOwned::to_owned),
            source_path: asset_source_path(asset).map(ToOwned::to_owned),
            sub_assets: sorted_sub_assets(&asset.sub_assets)
                .into_iter()
                .map(TreeSubAssetRow::from_sub_asset)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct TreeSubAssetRow {
    kind: String,
    name: String,
    path: String,
    ownership: String,
}

impl TreeSubAssetRow {
    fn from_sub_asset(asset: &SubAsset) -> Self {
        Self {
            kind: asset.kind.to_string(),
            name: asset.name.clone(),
            path: asset.path.to_string(),
            ownership: format_ownership(asset.ownership).to_owned(),
        }
    }
}

fn kind_header(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Plugin => "Plugins",
        AssetKind::Skill => "Skills",
        AssetKind::Agent => "Agents",
        AssetKind::Mcp => "MCPs",
        AssetKind::Hook => "Hooks",
        AssetKind::Workflow => "Workflows",
        AssetKind::Instruction => "Instructions",
    }
}

fn sorted_sub_assets(sub_assets: &[SubAsset]) -> Vec<&SubAsset> {
    let mut sub_assets: Vec<_> = sub_assets.iter().collect();
    sub_assets.sort_by(|left, right| {
        left.kind
            .to_string()
            .cmp(&right.kind.to_string())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.path.cmp(&right.path))
    });
    sub_assets
}

fn format_ownership(ownership: SubAssetOwnership) -> &'static str {
    match ownership {
        SubAssetOwnership::Parent => "parent",
        SubAssetOwnership::Standalone => "standalone",
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use chrono::Utc;
    use cpm_types::{AssetOwnership, AssetSource, GlobalClaim, GlobalLockfile, Lockfile, Scope};

    use super::*;

    fn make_asset(name: &str, scope: Scope) -> ResolvedAsset {
        ResolvedAsset {
            name: name.to_owned(),
            kind: AssetKind::Skill,
            source: AssetSource {
                url: Some(format!("https://example.com/{name}")),
                rev: None,
                path: Some(Utf8PathBuf::from(format!("skills/{name}"))),
                groups: if scope == Scope::Global {
                    vec!["default".to_owned(), "dev".to_owned()].into()
                } else {
                    "default".into()
                },
                scope,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: format!("rev-{name}"),
            resolved_date: Utc::now(),
            hash: format!("sha256:{name}"),
            scope,
            ownership: AssetOwnership::Upstream,
            files: vec![Utf8PathBuf::from(format!("{name}/SKILL.md")).into()],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        }
    }

    #[test]
    fn collect_visible_assets_includes_global_claims_not_in_local_lockfile() {
        let mut lockfile = Lockfile::new();
        lockfile.skills.push(make_asset("local-only", Scope::Local));

        let global_asset = make_asset("shared-global", Scope::Global);
        let mut global_lockfile = GlobalLockfile::new();
        global_lockfile.claims.push(GlobalClaim::new(
            Utf8PathBuf::from("/repos/other"),
            global_asset,
        ));

        let assets = collect_visible_assets(&lockfile, &global_lockfile);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|entry| {
            entry.asset.name == "shared-global"
                && entry.claimed_by.as_deref() == Some("/repos/other")
        }));
    }
}
