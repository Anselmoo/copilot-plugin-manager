//! `cpm export` — emit derived runtime config artifacts.

use std::path::Path;

use clap::{Args, Subcommand};
use cpm_core::{
    config::load_runtime_config,
    installer::cloud_agent_mcp_config,
    project::{load_lockfile, load_manifest, rewrite_mcp_source},
    CpmError,
};
use cpm_types::{AssetKind, Scope};

use super::find_locked_asset;

#[derive(Debug, Args)]
pub struct ExportArgs {
    #[command(subcommand)]
    pub command: ExportCommand,
}

#[derive(Debug, Subcommand)]
pub enum ExportCommand {
    /// Export MCP config in cloud-agent JSON format.
    McpCloud(McpCloudExportArgs),
}

#[derive(Debug, Args)]
pub struct McpCloudExportArgs {
    /// Export a single named MCP instead of all locked MCPs.
    pub name: Option<String>,

    /// Restrict to local-scope MCPs.
    #[arg(long, conflicts_with = "global")]
    pub local: bool,

    /// Restrict to global-scope MCPs.
    #[arg(long, conflicts_with = "local")]
    pub global: bool,
}

pub async fn run(args: ExportCommand) -> Result<(), CpmError> {
    match args {
        ExportCommand::McpCloud(options) => export_mcp_cloud(options),
    }
}

fn export_mcp_cloud(args: McpCloudExportArgs) -> Result<(), CpmError> {
    let manifest = load_manifest(Path::new("cpm.toml"))?;
    let runtime = load_runtime_config(&manifest)?;
    let lockfile = load_lockfile(Path::new("cpm.lock"))?;
    let scope = if args.local {
        Some(Scope::Local)
    } else if args.global {
        Some(Scope::Global)
    } else {
        None
    };

    let selected: Vec<_> = if let Some(name) = args.name {
        vec![find_locked_asset(&lockfile, AssetKind::Mcp, &name, scope)
            .ok_or(CpmError::AssetNotFound { name })?]
    } else {
        lockfile
            .all_assets()
            .filter(|asset| asset.kind == AssetKind::Mcp)
            .filter(|asset| scope.map(|wanted| asset.scope == wanted).unwrap_or(true))
            .collect()
    };

    let rewritten: Vec<_> = selected
        .into_iter()
        .cloned()
        .map(|mut asset| {
            asset.source = rewrite_mcp_source(&asset.source, &runtime.source_rules);
            asset
        })
        .collect();

    let json = cloud_agent_mcp_config(rewritten.iter().collect::<Vec<_>>())?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}
