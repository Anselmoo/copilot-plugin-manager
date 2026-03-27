//! `cpm list` — list installed assets.

use clap::Args;
use cpm_core::{
    paths::portable_path_string,
    project::{load_global_lockfile, load_lockfile},
    CpmError,
};
use cpm_types::{
    AssetKind, GlobalLockfile, Lockfile, ResolvedAsset, Scope, SubAsset, SubAssetOwnership,
};
use serde::Serialize;
use std::collections::HashSet;

use super::{
    asset_install_target, asset_source_path, asset_source_url, display_groups,
    effective_asset_scope, format_sub_asset_summary, json_group, json_groups, json_rev,
    kind_selected, style_asset_heading, style_label, KindSelection,
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
    let global_lockfile = load_global_lockfile()?;
    let selection = KindSelection {
        plugin: args.plugin,
        skill: args.skill,
        agent: args.agent,
        mcp: args.mcp,
        hook: args.hook,
        workflow: args.workflow,
        instruction: args.instruction,
    };
    let mut assets = collect_visible_assets(&lockfile, &global_lockfile, selection, args.scope);

    assets.sort_by(|left, right| {
        asset_kind_rank(left.asset.kind)
            .cmp(&asset_kind_rank(right.asset.kind))
            .then_with(|| left.asset.name.cmp(&right.asset.name))
            .then_with(|| {
                effective_asset_scope(&left.asset)
                    .to_string()
                    .cmp(&effective_asset_scope(&right.asset).to_string())
            })
            .then_with(|| left.claimed_by.cmp(&right.claimed_by))
    });

    if args.json {
        if assets.is_empty() {
            println!("[]");
            return Ok(());
        }
        let rows: Vec<_> = assets.iter().map(ListAssetRow::from_entry).collect();
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
        print_asset(&asset);
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct ListedAsset {
    asset: ResolvedAsset,
    claimed_by: Option<String>,
}

fn collect_visible_assets(
    lockfile: &Lockfile,
    global_lockfile: &GlobalLockfile,
    selection: KindSelection,
    scope: Option<ScopeArg>,
) -> Vec<ListedAsset> {
    let matches_scope = |asset: &ResolvedAsset| {
        scope
            .map(|requested| effective_asset_scope(asset) == Scope::from(requested))
            .unwrap_or(true)
    };

    let mut seen = HashSet::new();
    let mut assets = Vec::new();

    for asset in lockfile
        .all_assets()
        .filter(|asset| kind_selected(asset.kind, selection))
        .filter(|asset| matches_scope(asset))
    {
        seen.insert(asset_identity_key(asset));
        assets.push(ListedAsset {
            asset: asset.clone(),
            claimed_by: None,
        });
    }

    for claim in &global_lockfile.claims {
        let asset = &claim.asset;
        if !kind_selected(asset.kind, selection) || !matches_scope(asset) {
            continue;
        }
        let key = asset_identity_key(asset);
        if seen.insert(key) {
            assets.push(ListedAsset {
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

fn print_asset(entry: &ListedAsset) {
    let asset = &entry.asset;
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
    if let Some(groups) = display_groups(&asset.source.groups) {
        println!("  {} {}", style_label("groups"), groups);
    }
    if let Some(claimed_by) = &entry.claimed_by {
        println!("  {} {}", style_label("claimed-by"), claimed_by);
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
    groups: Option<Vec<String>>,
    claimed_by: Option<String>,
    rev: Option<String>,
    install_target: String,
    source_url: Option<String>,
    source_path: Option<String>,
    sub_assets: Vec<ListSubAssetRow>,
}

impl ListAssetRow {
    fn from_entry(entry: &ListedAsset) -> Self {
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

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use chrono::Utc;
    use cpm_types::{AssetOwnership, AssetSource, GlobalClaim, GlobalLockfile, Lockfile};

    use super::*;

    fn selection_all() -> KindSelection {
        KindSelection {
            plugin: false,
            skill: false,
            agent: false,
            mcp: false,
            hook: false,
            workflow: false,
            instruction: false,
        }
    }

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

        let assets = collect_visible_assets(&lockfile, &global_lockfile, selection_all(), None);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|entry| {
            entry.asset.name == "shared-global"
                && entry.claimed_by.as_deref() == Some("/repos/other")
        }));
    }
}
