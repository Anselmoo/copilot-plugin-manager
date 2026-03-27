//! `cpm show` — show full details of a single asset.

use clap::Args;
use cpm_core::{project::load_lockfile, CpmError};
use cpm_types::{
    EnvSpec, EnvValue, LicenseInfo, LockedFile, McpTransport, ResolvedAsset, SubAsset,
};
use serde::Serialize;

use super::{
    asset_install_target, asset_source_path, asset_source_url, format_sub_asset_summary,
    json_group, json_rev, style_asset_heading, style_label,
};

/// Arguments for `cpm show`.
#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Asset name.
    pub name: String,

    /// Emit JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: ShowArgs) -> Result<(), CpmError> {
    let lockfile = load_lockfile(std::path::Path::new("cpm.lock"))?;
    let mut matches: Vec<_> = lockfile
        .all_assets()
        .filter(|asset| asset.name == args.name)
        .collect();

    matches.sort_by(|left, right| {
        left.kind
            .to_string()
            .cmp(&right.kind.to_string())
            .then_with(|| left.scope.to_string().cmp(&right.scope.to_string()))
    });

    if matches.is_empty() {
        return Err(CpmError::AssetNotFound { name: args.name });
    }

    if args.json {
        let rows: Vec<_> = matches
            .iter()
            .map(|asset| ShowAssetRow::from_asset(asset))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    for (index, asset) in matches.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!(
            "{}",
            style_asset_heading(asset.kind, asset.scope, &asset.name)
        );
        println!("{} {}", style_label("group"), asset.source.group);
        println!(
            "{} {}",
            style_label("rev"),
            display_optional(&asset.resolved_rev)
        );
        println!("{} {}", style_label("hash"), asset.hash);
        println!(
            "{} {}",
            style_label("source-url"),
            display_option(asset_source_url(asset))
        );
        println!(
            "{} {}",
            style_label("source-path"),
            display_option(asset_source_path(asset))
        );
        println!(
            "{} {}",
            style_label("transport"),
            format_transport(asset.source.transport.as_ref())
        );
        println!(
            "{} {}",
            style_label("args"),
            format_list(&asset.source.args)
        );
        println!("{} {}", style_label("env"), format_env(&asset.source.env));
        println!("{} {}", style_label("files"), format_files(&asset.files));
        println!(
            "{} {}",
            style_label("installed"),
            asset_install_target(asset)
        );
        if !asset.sub_assets.is_empty() {
            println!("{} {}", style_label("sub-assets"), asset.sub_assets.len());
            for sub_asset in sorted_sub_assets(&asset.sub_assets) {
                println!("  {}", format_sub_asset(sub_asset));
            }
        }
        if let Some(license) = &asset.license {
            println!("{} {}", style_label("license.spdx"), license.spdx);
            println!("{} {}", style_label("license.verified"), license.verified);
            println!(
                "{} {}",
                style_label("license.url"),
                display_option(license.url.as_deref())
            );
        }
        println!(
            "{} {}",
            style_label("bin-path"),
            display_option(asset.bin_path.as_ref().map(|path| path.as_str()))
        );
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct ShowAssetRow {
    name: String,
    kind: String,
    scope: String,
    group: Option<String>,
    rev: Option<String>,
    hash: String,
    source_url: Option<String>,
    source_path: Option<String>,
    transport: Option<McpTransport>,
    args: Vec<String>,
    env: Vec<EnvSpec>,
    files: Vec<LockedFile>,
    install_target: String,
    sub_assets: Vec<ShowSubAssetRow>,
    license: Option<LicenseInfo>,
    bin_path: Option<String>,
}

impl ShowAssetRow {
    fn from_asset(asset: &ResolvedAsset) -> Self {
        Self {
            name: asset.name.clone(),
            kind: asset.kind.to_string(),
            scope: asset.scope.to_string(),
            group: json_group(&asset.source.group),
            rev: json_rev(&asset.resolved_rev),
            hash: asset.hash.clone(),
            source_url: asset_source_url(asset).map(ToOwned::to_owned),
            source_path: asset_source_path(asset).map(ToOwned::to_owned),
            transport: asset.source.transport.clone(),
            args: asset.source.args.clone(),
            env: asset.source.env.clone(),
            files: asset.files.clone(),
            install_target: asset_install_target(asset),
            sub_assets: sorted_sub_assets(&asset.sub_assets)
                .into_iter()
                .map(ShowSubAssetRow::from_sub_asset)
                .collect(),
            license: asset.license.clone(),
            bin_path: asset.bin_path.as_ref().map(|path| path.as_str().to_owned()),
        }
    }
}

#[derive(Debug, Serialize)]
struct ShowSubAssetRow {
    kind: String,
    name: String,
    path: String,
    ownership: String,
}

impl ShowSubAssetRow {
    fn from_sub_asset(asset: &SubAsset) -> Self {
        Self {
            kind: asset.kind.to_string(),
            name: asset.name.clone(),
            path: asset.path.to_string(),
            ownership: format_ownership(asset).to_owned(),
        }
    }
}

fn display_optional(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

fn display_option(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn format_transport(transport: Option<&McpTransport>) -> String {
    match transport {
        Some(McpTransport::Http { url }) => format!("type=http ({url})"),
        Some(McpTransport::Sse { url }) => format!("type=sse ({url})"),
        Some(McpTransport::Npx { package, .. }) => format!("type=stdio runner=npx ({package})"),
        Some(McpTransport::Uvx { package, .. }) => format!("type=stdio runner=uvx ({package})"),
        Some(McpTransport::Docker { image, .. }) => {
            format!("type=stdio runner=docker ({image})")
        }
        Some(McpTransport::Binary { url, bin, .. }) => {
            format!("type=stdio runner=binary ({url}, bin={bin})")
        }
        Some(McpTransport::Path { path, .. }) => {
            format!("type=stdio runner=local ({})", path.display())
        }
        Some(McpTransport::Script { command, .. }) => {
            format!("type=stdio runner=command ({command})")
        }
        None => "-".to_owned(),
    }
}

fn format_list(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(", ")
    }
}

fn format_files(values: &[cpm_types::LockedFile]) -> String {
    if values.is_empty() {
        "-".to_owned()
    } else {
        values
            .iter()
            .map(|file| match &file.sha256 {
                Some(sha256) => format!("{} (sha256:{sha256})", file.path),
                None => file.path.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn format_env(values: &[cpm_types::EnvSpec]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }

    values
        .iter()
        .map(|spec| match &spec.value {
            EnvValue::Literal(value) => format!("{}={value}", spec.key),
            EnvValue::FromEnv(var) => format!("{}=${var}", spec.key),
        })
        .collect::<Vec<_>>()
        .join(", ")
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
    format_sub_asset_summary(asset)
}

fn format_ownership(asset: &SubAsset) -> &'static str {
    match asset.ownership {
        cpm_types::SubAssetOwnership::Parent => "parent",
        cpm_types::SubAssetOwnership::Standalone => "standalone",
    }
}
