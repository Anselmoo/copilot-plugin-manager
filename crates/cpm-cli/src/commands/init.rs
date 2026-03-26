//! `cpm init` — initialise a new cpm project in the current directory.
//!
//! Creates a minimal `cpm.toml` and `cpm.lock` if they do not exist, similar
//! to `uv init` or `cargo init`.

use std::path::Path;

use clap::Args;
use cpm_core::{
    project::{write_lockfile, write_manifest},
    CpmError,
};
use cpm_types::{Lockfile, Manifest, PartialSettings};
use tracing::info;

/// Arguments for `cpm init`.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Project name (defaults to the current directory name).
    #[arg(long)]
    pub name: Option<String>,

    /// Default install scope for this project.
    #[arg(long, value_enum, default_value = "local")]
    pub scope: ScopeArg,

    /// Overwrite existing `cpm.toml` if it already exists.
    #[arg(long)]
    pub force: bool,
}

/// Scope argument.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScopeArg {
    Local,
    Global,
}

pub async fn run(args: InitArgs) -> Result<(), CpmError> {
    let manifest_path = Path::new("cpm.toml");
    let lockfile_path = Path::new("cpm.lock");

    // Determine project name.
    let project_name = args.name.unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "my-project".to_owned())
    });

    // Refuse to overwrite an existing manifest unless --force is passed.
    if manifest_path.exists() && !args.force {
        println!("✓ cpm.toml already exists (use --force to overwrite).");
        return Ok(());
    }

    let default_scope = match args.scope {
        ScopeArg::Local => cpm_types::Scope::Local,
        ScopeArg::Global => cpm_types::Scope::Global,
    };

    // Build a minimal manifest.
    let manifest = Manifest {
        package: Some(cpm_types::PackageMetadata {
            name: project_name.clone(),
            version: "0.1.0".to_owned(),
            description: None,
            license: None,
            authors: None,
            repository: None,
            created: None,
        }),
        settings: PartialSettings {
            default_scope: Some(default_scope),
            ..PartialSettings::default()
        },
        ..Manifest::default()
    };

    write_manifest(manifest_path, &manifest)?;
    info!("created cpm.toml");

    // Write an empty lockfile.
    if !lockfile_path.exists() {
        write_lockfile(lockfile_path, &Lockfile::new())?;
        info!("created cpm.lock");
    }

    println!("✓ Initialised cpm project '{project_name}'.");
    println!("  manifest : cpm.toml");
    println!("  lockfile : cpm.lock");
    println!("  scope    : {default_scope}");
    println!();
    println!("Next steps:");
    println!("  cpm add <url> --skill     # add a skill");
    println!("  cpm add <url> --plugin    # add a plugin");
    println!("  cpm add <url> --agent     # add an agent");
    println!("  cpm add <url> --mcp       # add an MCP server");
    println!("  cpm add <url> --hook      # add a hook bundle");
    println!("  cpm add <url> --workflow  # add a workflow");
    println!("  cpm sync                  # install everything");

    Ok(())
}
