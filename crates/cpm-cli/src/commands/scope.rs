//! `cpm scope` — get or set the default install scope.

use clap::{Args, Subcommand};
use cpm_core::{
    config::load_runtime_config,
    project::{load_manifest, write_manifest},
    CpmError,
};
use cpm_types::Scope;

/// Arguments for `cpm scope`.
#[derive(Debug, Args)]
pub struct ScopeArgs {
    #[command(subcommand)]
    pub command: ScopeCommand,
}

/// `cpm scope` subcommands.
#[derive(Debug, Subcommand)]
pub enum ScopeCommand {
    /// Set or show the repository-level default scope.
    Default {
        /// New scope value (`local` or `global`). Shows current value if omitted.
        value: Option<String>,
    },
}

pub async fn run(cmd: ScopeCommand) -> Result<(), CpmError> {
    match cmd {
        ScopeCommand::Default { value: Some(v) } => {
            let scope = parse_scope(&v)?;
            let manifest_path = std::path::Path::new("cpm.toml");
            let mut manifest = load_manifest(manifest_path)?;
            manifest.settings.default_scope = Some(scope);
            write_manifest(manifest_path, &manifest)?;
            println!("Set repo default scope to '{scope}'");
        }
        ScopeCommand::Default { value: None } => {
            let manifest = load_manifest(std::path::Path::new("cpm.toml"))?;
            let runtime = load_runtime_config(&manifest)?;
            println!("Default scope: {}", runtime.settings.default_scope);
        }
    }
    Ok(())
}

fn parse_scope(value: &str) -> Result<Scope, CpmError> {
    value
        .parse::<Scope>()
        .map_err(|reason| CpmError::InvalidConfig {
            key: "settings.default_scope".to_owned(),
            reason,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scope_accepts_supported_values() {
        assert_eq!(parse_scope("local").expect("scope"), Scope::Local);
        assert_eq!(parse_scope("global").expect("scope"), Scope::Global);
    }

    #[test]
    fn parse_scope_rejects_unknown_values() {
        let err = parse_scope("workspace").expect_err("invalid scope");
        assert!(matches!(err, CpmError::InvalidConfig { .. }));
    }
}
