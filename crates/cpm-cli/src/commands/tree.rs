//! `cpm tree` — show the dependency tree.

use clap::Args;
use cpm_core::{project::load_lockfile, CpmError};
use cpm_types::{AssetKind, ResolvedAsset, SubAsset, SubAssetOwnership};
use serde::Serialize;

use super::{
    asset_install_target, asset_source_path, asset_source_url, format_sub_asset_summary,
    json_group, json_rev, style_asset_heading, style_heading, style_label,
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
            let mut assets: Vec<_> = lockfile
                .all_assets()
                .filter(|asset| asset.kind == kind)
                .collect();
            assets.sort_by(|left, right| left.name.cmp(&right.name));
            (!assets.is_empty()).then(|| TreeSection {
                kind: kind_header(kind).to_owned(),
                assets: assets
                    .into_iter()
                    .map(TreeAssetRow::from_asset)
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
        println!("{}", serde_json::to_string_pretty(&sections)?);
        return Ok(());
    }

    if lockfile.all_assets().next().is_none() {
        println!("No locked assets found.");
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
        let mut assets: Vec<_> = lockfile
            .all_assets()
            .filter(|asset| asset.kind == kind)
            .collect();
        assets.sort_by(|left, right| left.name.cmp(&right.name));
        if assets.is_empty() {
            continue;
        }

        if !first_section {
            println!();
        }
        first_section = false;
        println!("{}", style_heading(kind_header(kind)));
        for asset in assets {
            println!(
                "└── {}",
                style_asset_heading(asset.kind, asset.scope, &asset.name)
            );
            println!(
                "    {} {}",
                style_label("installed"),
                asset_install_target(asset)
            );
            if asset.source.group != "default" {
                println!("    {} {}", style_label("group"), asset.source.group);
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
    rev: Option<String>,
    install_target: String,
    source_url: Option<String>,
    source_path: Option<String>,
    sub_assets: Vec<TreeSubAssetRow>,
}

impl TreeAssetRow {
    fn from_asset(asset: &ResolvedAsset) -> Self {
        Self {
            name: asset.name.clone(),
            kind: asset.kind.to_string(),
            scope: asset.scope.to_string(),
            group: json_group(&asset.source.group),
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
