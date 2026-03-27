//! `cpm list` — list installed assets.

use clap::Args;
use cpm_core::{project::load_lockfile, CpmError};
use cpm_types::{AssetKind, ResolvedAsset, Scope, SubAsset, SubAssetOwnership};
use serde::Serialize;

use super::{
    asset_install_target, asset_source_path, asset_source_url, effective_asset_scope,
    format_sub_asset_summary, json_group, json_rev, kind_selected, style_asset_heading,
    style_label, KindSelection,
};

/// Arguments for `cpm list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    #[arg(long, group = "kind")]
    pub plugin: bool,
    #[arg(long, group = "kind")]
    pub skill: bool,
    #[arg(long, group = "kind")]
    pub agent: bool,
    #[arg(long, group = "kind")]
    pub mcp: bool,
    #[arg(long, group = "kind")]
    pub hook: bool,
    #[arg(long, group = "kind")]
    pub workflow: bool,
    #[arg(long, group = "kind")]
    pub instruction: bool,

    /// Filter by scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,

    /// Emit JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

/// Scope argument.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScopeArg {
    Local,
    Global,
}

pub async fn run(args: ListArgs) -> Result<(), CpmError> {
    let lockfile = load_lockfile(std::path::Path::new("cpm.lock"))?;
    let selection = KindSelection {
        plugin: args.plugin,
        skill: args.skill,
        agent: args.agent,
        mcp: args.mcp,
        hook: args.hook,
        workflow: args.workflow,
        instruction: args.instruction,
    };
    let mut assets: Vec<_> = lockfile
        .all_assets()
        .filter(|asset| kind_selected(asset.kind, selection))
        .filter(|asset| {
            args.scope
                .map(|scope| effective_asset_scope(asset) == Scope::from(scope))
                .unwrap_or(true)
        })
        .collect();

    assets.sort_by(|left, right| {
        asset_kind_rank(left.kind)
            .cmp(&asset_kind_rank(right.kind))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| {
                effective_asset_scope(left)
                    .to_string()
                    .cmp(&effective_asset_scope(right).to_string())
            })
    });

    if args.json {
        if assets.is_empty() {
            println!("[]");
            return Ok(());
        }
        let rows: Vec<_> = assets
            .iter()
            .map(|asset| ListAssetRow::from_asset(asset))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if assets.is_empty() {
        println!("No assets matched the current filters.");
        return Ok(());
    }

    for (index, asset) in assets.into_iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_asset(asset);
    }

    Ok(())
}

fn print_asset(asset: &ResolvedAsset) {
    println!(
        "{}",
        style_asset_heading(asset.kind, effective_asset_scope(asset), &asset.name)
    );
    println!(
        "  {} {}",
        style_label("installed"),
        asset_install_target(asset)
    );
    for line in source_lines(asset) {
        println!("  {line}");
    }
    if asset.source.group != "default" {
        println!("  {} {}", style_label("group"), asset.source.group);
    }
    if !asset.resolved_rev.is_empty() {
        println!("  {} {}", style_label("rev"), asset.resolved_rev);
    }
    if !asset.sub_assets.is_empty() {
        println!("  {} {}", style_label("sub-assets"), asset.sub_assets.len());
        for sub_asset in sorted_sub_assets(&asset.sub_assets) {
            println!("    {}", format_sub_asset(sub_asset));
        }
    }
}

fn source_lines(asset: &ResolvedAsset) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(url) = asset_source_url(asset) {
        lines.push(format!("{} {url}", style_label("source-url")));
    }
    if let Some(path) = asset_source_path(asset) {
        lines.push(format!("{} {path}", style_label("source-path")));
    }
    if lines.is_empty() {
        lines.push(format!("{} -", style_label("source")));
    }
    lines
}

#[derive(Debug, Serialize)]
struct ListAssetRow {
    name: String,
    kind: String,
    scope: String,
    group: Option<String>,
    rev: Option<String>,
    install_target: String,
    source_url: Option<String>,
    source_path: Option<String>,
    sub_assets: Vec<ListSubAssetRow>,
}

impl ListAssetRow {
    fn from_asset(asset: &ResolvedAsset) -> Self {
        Self {
            name: asset.name.clone(),
            kind: asset.kind.to_string(),
            scope: effective_asset_scope(asset).to_string(),
            group: json_group(&asset.source.group),
            rev: json_rev(&asset.resolved_rev),
            install_target: asset_install_target(asset),
            source_url: asset_source_url(asset).map(ToOwned::to_owned),
            source_path: asset_source_path(asset).map(ToOwned::to_owned),
            sub_assets: sorted_sub_assets(&asset.sub_assets)
                .into_iter()
                .map(ListSubAssetRow::from_sub_asset)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ListSubAssetRow {
    kind: String,
    name: String,
    path: String,
    ownership: String,
}

impl ListSubAssetRow {
    fn from_sub_asset(asset: &SubAsset) -> Self {
        Self {
            kind: asset.kind.to_string(),
            name: asset.name.clone(),
            path: asset.path.to_string(),
            ownership: format_ownership(asset.ownership).to_owned(),
        }
    }
}

fn asset_kind_rank(kind: AssetKind) -> u8 {
    match kind {
        AssetKind::Plugin => 0,
        AssetKind::Skill => 1,
        AssetKind::Agent => 2,
        AssetKind::Mcp => 3,
        AssetKind::Hook => 4,
        AssetKind::Workflow => 5,
        AssetKind::Instruction => 6,
    }
}

impl From<ScopeArg> for Scope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Local => Scope::Local,
            ScopeArg::Global => Scope::Global,
        }
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

fn format_sub_asset(asset: &SubAsset) -> String {
    format!("↳ {}", format_sub_asset_summary(asset))
}

fn format_ownership(ownership: SubAssetOwnership) -> &'static str {
    match ownership {
        SubAssetOwnership::Parent => "parent",
        SubAssetOwnership::Standalone => "standalone",
    }
}
