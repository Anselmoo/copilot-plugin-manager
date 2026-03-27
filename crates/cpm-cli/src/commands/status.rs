//! `cpm status` — show drift between manifest, lockfile, and disk.

use clap::Args;
use cpm_core::project::{load_global_lockfile, load_lockfile, load_manifest};
use cpm_core::status::{check_status_with_global_lock, AssetStatus};
use cpm_core::CpmError;
use serde::Serialize;

use super::{style_error, style_heading, style_label, style_success, style_warning};

/// Arguments for `cpm status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Emit JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: StatusArgs) -> Result<(), CpmError> {
    let repo_root = std::env::current_dir()?;
    let manifest = load_manifest(std::path::Path::new("cpm.toml"))?;
    let lockfile = load_lockfile(std::path::Path::new("cpm.lock"))?;
    let global_lockfile = load_global_lockfile()?;
    let statuses =
        check_status_with_global_lock(&manifest, &lockfile, &global_lockfile, &repo_root)?;

    if args.json {
        let rows: Vec<_> = statuses.iter().map(StatusRow::from_status).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if statuses.is_empty() {
        println!("{} Everything is up to date.", style_success("✓"));
        return Ok(());
    }

    println!("{}", style_heading("Status issues:"));
    for s in &statuses {
        match s {
            AssetStatus::Unlocked { name } => {
                println!(
                    "{} {name}: in manifest but not locked — run `cpm lock` or `cpm sync`",
                    style_warning("⚠")
                );
            }
            AssetStatus::Drift { name, detail } => {
                println!("{} {name}: {detail}", style_error("✖"));
            }
            AssetStatus::Stale {
                name,
                locked_rev,
                requested_rev,
            } => {
                println!(
                    "{} {name}: locked at {locked_rev}, manifest wants {requested_rev}",
                    style_warning("↻")
                );
            }
            AssetStatus::GlobalState { name, detail } => {
                println!("{} {name}: {detail}", style_error("!"));
            }
        }
    }

    if statuses.iter().any(|status| {
        matches!(
            status,
            AssetStatus::Drift { .. } | AssetStatus::GlobalState { .. }
        )
    }) {
        println!();
        println!(
            "{} `cpm doctor` for hash verification details.",
            style_label("Tip")
        );
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct StatusRow {
    status: String,
    name: String,
    detail: Option<String>,
    locked_rev: Option<String>,
    requested_rev: Option<String>,
    recommendation: Option<String>,
}

impl StatusRow {
    fn from_status(status: &AssetStatus) -> Self {
        match status {
            AssetStatus::Unlocked { name } => Self {
                status: "unlocked".to_owned(),
                name: name.clone(),
                detail: Some("in manifest but not locked".to_owned()),
                locked_rev: None,
                requested_rev: None,
                recommendation: Some("run `cpm lock` or `cpm sync`".to_owned()),
            },
            AssetStatus::Drift { name, detail } => Self {
                status: "drift".to_owned(),
                name: name.clone(),
                detail: Some(detail.clone()),
                locked_rev: None,
                requested_rev: None,
                recommendation: Some("run `cpm doctor` or `cpm sync`".to_owned()),
            },
            AssetStatus::Stale {
                name,
                locked_rev,
                requested_rev,
            } => Self {
                status: "stale".to_owned(),
                name: name.clone(),
                detail: Some("manifest and lockfile disagree".to_owned()),
                locked_rev: Some(locked_rev.clone()),
                requested_rev: Some(requested_rev.clone()),
                recommendation: Some("run `cpm lock` or `cpm sync`".to_owned()),
            },
            AssetStatus::GlobalState { name, detail } => Self {
                status: "global_state".to_owned(),
                name: name.clone(),
                detail: Some(detail.clone()),
                locked_rev: None,
                requested_rev: None,
                recommendation: Some(
                    "run `cpm doctor` or inspect the global lock state".to_owned(),
                ),
            },
        }
    }
}
