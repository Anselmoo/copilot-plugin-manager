//! All CLI subcommands for `cpm`.

use std::{collections::HashSet, fmt::Display, io::IsTerminal, path::Path};

use crate::progress::{OperationKind, OperationStatus, ProgressReporter};
use chrono::Utc;
use clap::{
    builder::styling::{AnsiColor, Effects, Styles},
    Parser, Subcommand,
};
use cpm_core::{
    auth as core_auth,
    config::{build_http_client, load_runtime_config},
    fetcher::sha256_hex,
    installer::{copilot_mcp_config_path, install_dir, remove_asset},
    paths::portable_path_string,
    plugin_delegate::PluginDelegate,
    plugin_index::{
        find_installed_plugin_by_name, hash_installed_plugin_manifest, installed_plugin_request,
        preferred_plugin_install_root, InstalledPlugin,
    },
    project::{
        apply_manifest, install_resolved_asset, load_lockfile, load_manifest, write_lockfile,
        write_manifest, ApplyOptions,
    },
    source::normalize_asset_source,
    CpmError,
};
use cpm_types::{
    AssetKind, AssetOwnership, AssetSource, Lockfile, Manifest, PluginMeta, ResolvedAsset, Scope,
    SubAsset, SubAssetOwnership,
};

mod add;
mod auth;
mod cache;
mod doctor;
mod init;
mod list;
mod lock;
mod overview;
mod remove;
mod reset;
mod run;
mod scope;
mod show;
mod status;
mod sync;
mod tree;
mod update;

pub use add::AddArgs;
pub use auth::AuthArgs;
pub use cache::CacheArgs;
pub use doctor::DoctorArgs;
pub use init::InitArgs;
pub use list::ListArgs;
pub use lock::LockArgs;
pub use overview::OverviewArgs;
pub use remove::RemoveArgs;
pub use reset::ResetArgs;
pub use run::RunArgs;
pub use scope::ScopeArgs;
pub use show::ShowArgs;
pub use status::StatusArgs;
pub use sync::SyncArgs;
pub use tree::TreeArgs;
pub use update::UpdateArgs;

const CLI_STYLES: Styles = Styles::styled()
    .header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default())
    .placeholder(AnsiColor::Cyan.on_default());

const CLI_AFTER_HELP: &str = "\
Getting started:
  init      Create a new cpm.toml + cpm.lock in the current directory
  add       Add an asset to the manifest and resolve it
  sync      Install everything recorded in cpm.lock

Manage assets:
  remove    Remove a managed asset
  promote   Move an asset from local to global scope
  demote    Move an asset from global to local scope
  update    Update one or all assets
  run       Fetch and run an asset without installing it

Inspect & diagnose:
  overview  See the combined manifest, lockfile, and disk view
  list      List installed assets
  show      Show details for one asset
  tree      Show the dependency tree
  doctor    Verify installed file hashes
  status    Show manifest/lockfile/disk drift

Maintenance:
  lock      Resolve without installing
  reset     Remove managed state and installed assets
  cache     Inspect or clean the download cache
  auth      Manage authentication tokens
  scope     Get or set the default install scope";

/// cpm — GitHub Copilot asset manager
#[derive(Debug, Parser)]
#[command(
    name = "cpm",
    version,
    about = "Copilot Plugin Manager — manage plugins, skills, agents, MCPs, hooks, workflows, and instructions",
    long_about = None,
    after_help = CLI_AFTER_HELP,
    styles = CLI_STYLES,
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Initialise a new cpm project in the current directory.
    #[command(display_order = 1)]
    Init(InitArgs),
    /// Add an asset to cpm.toml and resolve it.
    #[command(display_order = 2)]
    Add(AddArgs),
    /// Install everything recorded in cpm.lock.
    #[command(display_order = 3)]
    Sync(SyncArgs),
    /// Remove an asset.
    #[command(display_order = 10)]
    Remove(RemoveArgs),
    /// Promote an asset from local → global scope.
    #[command(display_order = 11)]
    Promote {
        /// Asset name.
        name: String,
        #[arg(long)]
        plugin: bool,
        #[arg(long)]
        skill: bool,
        #[arg(long)]
        agent: bool,
        #[arg(long)]
        mcp: bool,
        #[arg(long)]
        hook: bool,
        #[arg(long)]
        workflow: bool,
        #[arg(long)]
        instruction: bool,
    },
    /// Demote an asset from global → local scope.
    #[command(display_order = 12)]
    Demote {
        /// Asset name.
        name: String,
        #[arg(long)]
        plugin: bool,
        #[arg(long)]
        skill: bool,
        #[arg(long)]
        agent: bool,
        #[arg(long)]
        mcp: bool,
        #[arg(long)]
        hook: bool,
        #[arg(long)]
        workflow: bool,
        #[arg(long)]
        instruction: bool,
    },
    /// Update one or all assets to their latest versions.
    #[command(display_order = 13)]
    Update(UpdateArgs),
    /// Resolve without installing (CI use).
    #[command(display_order = 30)]
    Lock(LockArgs),
    /// Show a consolidated view of managed and installed assets.
    #[command(display_order = 20)]
    Overview(OverviewArgs),
    /// List installed assets.
    #[command(display_order = 21)]
    List(ListArgs),
    /// Show full details of a single asset.
    #[command(display_order = 22)]
    Show(ShowArgs),
    /// Show the dependency tree.
    #[command(display_order = 23)]
    Tree(TreeArgs),
    /// Verify all installed file hashes match the lockfile.
    #[command(display_order = 24)]
    Doctor(DoctorArgs),
    /// Show drift between manifest, lockfile, and disk.
    #[command(display_order = 25)]
    Status(StatusArgs),
    /// Reset installed assets and/or manifest state.
    #[command(display_order = 31)]
    Reset(ResetArgs),
    /// Manage the cpm cache.
    #[command(display_order = 32)]
    Cache(CacheArgs),
    /// Fetch and run an asset without installing it.
    #[command(display_order = 14)]
    Run(RunArgs),
    /// Manage authentication tokens.
    #[command(display_order = 33)]
    Auth(AuthArgs),
    /// Get or set the default scope.
    #[command(display_order = 34)]
    Scope(ScopeArgs),
}

impl Cli {
    /// Dispatch the parsed command to its handler.
    pub async fn run(self) -> Result<(), cpm_core::CpmError> {
        match self.command {
            Commands::Init(args) => init::run(args).await,
            Commands::Add(args) => add::run(args).await,
            Commands::Sync(args) => sync::run(args).await,
            Commands::Remove(args) => remove::run(args).await,
            Commands::Promote {
                name,
                plugin,
                skill,
                agent,
                mcp,
                hook,
                workflow,
                instruction,
            } => {
                run_scope_transition(
                    name,
                    resolve_required_kind(plugin, skill, agent, mcp, hook, workflow, instruction)?,
                    Scope::Global,
                )
                .await
            }
            Commands::Demote {
                name,
                plugin,
                skill,
                agent,
                mcp,
                hook,
                workflow,
                instruction,
            } => {
                run_scope_transition(
                    name,
                    resolve_required_kind(plugin, skill, agent, mcp, hook, workflow, instruction)?,
                    Scope::Local,
                )
                .await
            }
            Commands::Update(args) => update::run(args).await,
            Commands::Lock(args) => lock::run(args).await,
            Commands::Overview(args) => overview::run(args).await,
            Commands::List(args) => list::run(args).await,
            Commands::Show(args) => show::run(args).await,
            Commands::Tree(args) => tree::run(args).await,
            Commands::Doctor(args) => doctor::run(args).await,
            Commands::Status(args) => status::run(args).await,
            Commands::Reset(args) => reset::run(args).await,
            Commands::Cache(CacheArgs { command }) => cache::run(command).await,
            Commands::Run(args) => run::run(args).await,
            Commands::Auth(AuthArgs { command }) => auth::run(command).await,
            Commands::Scope(ScopeArgs { command }) => scope::run(command).await,
        }
    }
}

pub(super) fn resolve_required_kind(
    plugin: bool,
    skill: bool,
    agent: bool,
    mcp: bool,
    hook: bool,
    workflow: bool,
    instruction: bool,
) -> Result<AssetKind, CpmError> {
    match (plugin, skill, agent, mcp, hook, workflow, instruction) {
        (true, false, false, false, false, false, false) => Ok(AssetKind::Plugin),
        (false, true, false, false, false, false, false) => Ok(AssetKind::Skill),
        (false, false, true, false, false, false, false) => Ok(AssetKind::Agent),
        (false, false, false, true, false, false, false) => Ok(AssetKind::Mcp),
        (false, false, false, false, true, false, false) => Ok(AssetKind::Hook),
        (false, false, false, false, false, true, false) => Ok(AssetKind::Workflow),
        (false, false, false, false, false, false, true) => Ok(AssetKind::Instruction),
        _ => Err(CpmError::InvalidConfig {
            key: "kind".to_owned(),
            reason:
                "pass exactly one of --plugin, --skill, --agent, --mcp, --hook, --workflow, or --instruction"
                .to_owned(),
        }),
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct KindSelection {
    pub plugin: bool,
    pub skill: bool,
    pub agent: bool,
    pub mcp: bool,
    pub hook: bool,
    pub workflow: bool,
    pub instruction: bool,
}

pub(super) fn kind_selected(kind: AssetKind, selection: KindSelection) -> bool {
    if !(selection.plugin
        || selection.skill
        || selection.agent
        || selection.mcp
        || selection.hook
        || selection.workflow
        || selection.instruction)
    {
        return true;
    }

    matches!(
        (
            kind,
            selection.plugin,
            selection.skill,
            selection.agent,
            selection.mcp,
            selection.hook,
            selection.workflow,
            selection.instruction
        ),
        (
            AssetKind::Plugin,
            true,
            false,
            false,
            false,
            false,
            false,
            false
        ) | (
            AssetKind::Skill,
            false,
            true,
            false,
            false,
            false,
            false,
            false
        ) | (
            AssetKind::Agent,
            false,
            false,
            true,
            false,
            false,
            false,
            false
        ) | (
            AssetKind::Mcp,
            false,
            false,
            false,
            true,
            false,
            false,
            false
        ) | (
            AssetKind::Hook,
            false,
            false,
            false,
            false,
            true,
            false,
            false
        ) | (
            AssetKind::Workflow,
            false,
            false,
            false,
            false,
            false,
            true,
            false
        ) | (
            AssetKind::Instruction,
            false,
            false,
            false,
            false,
            false,
            false,
            true
        )
    )
}

pub(super) fn find_locked_asset<'a>(
    lockfile: &'a Lockfile,
    kind: AssetKind,
    name: &str,
    scope: Option<Scope>,
) -> Option<&'a cpm_types::ResolvedAsset> {
    lockfile.all_assets().find(|asset| {
        asset.kind == kind
            && asset.name == name
            && scope
                .map(|requested| asset.scope == requested)
                .unwrap_or(true)
    })
}

pub(super) fn asset_install_target(asset: &ResolvedAsset) -> String {
    let repo_root = Path::new(".");
    if asset.kind == AssetKind::Mcp {
        return normalized_display_path(&copilot_mcp_config_path(asset.scope, repo_root));
    }
    if asset.kind == AssetKind::Plugin && asset.files.is_empty() {
        let install_root = preferred_plugin_install_root(
            &asset.name,
            asset
                .plugin_meta
                .as_ref()
                .and_then(|meta| meta.registry.as_deref()),
        );
        return normalized_display_path(&install_root);
    }

    let root = install_dir(asset.kind, asset.scope, repo_root);
    if asset.files.is_empty() {
        return normalized_display_path(&root);
    }

    asset
        .files
        .iter()
        .map(|file| normalized_display_path(&root.join(file.path.as_std_path())))
        .collect::<Vec<_>>()
        .join(", ")
}

fn normalized_display_path(path: &Path) -> String {
    portable_path_string(path)
}

pub(super) fn asset_source_url(asset: &ResolvedAsset) -> Option<&str> {
    asset.source.url.as_deref()
}

pub(super) fn asset_source_path(asset: &ResolvedAsset) -> Option<&str> {
    asset.source.path.as_ref().map(|path| path.as_str())
}

pub(super) fn json_group(group: &str) -> Option<String> {
    (group != "default").then(|| group.to_owned())
}

pub(super) fn json_rev(rev: &str) -> Option<String> {
    (!rev.is_empty()).then(|| rev.to_owned())
}

pub(super) fn format_sub_asset_summary(asset: &SubAsset) -> String {
    format!(
        "{} {} [{}] path={}",
        asset.kind,
        asset.name,
        format_sub_asset_ownership(asset.ownership),
        asset.path
    )
}

pub(super) fn style_heading(text: &str) -> String {
    colorize(text, "1;36")
}

pub(super) fn style_label(text: &str) -> String {
    format!("{}:", colorize(text, "1"))
}

pub(super) fn style_success(text: &str) -> String {
    colorize(text, "32")
}

pub(super) fn style_warning(text: &str) -> String {
    colorize(text, "33")
}

pub(super) fn style_error(text: &str) -> String {
    colorize(text, "31")
}

fn style_scope_name(scope: &str) -> String {
    style_scope_name_with_colors(scope, output_colors_enabled())
}

fn style_scope_name_with_colors(scope: &str, colors_enabled: bool) -> String {
    match scope {
        "local" => colorize_with_mode("local", "1;34", colors_enabled),
        "global" => colorize_with_mode("global", "1;35", colors_enabled),
        other => colorize_with_mode(other, "1;34", colors_enabled),
    }
}

pub(super) fn style_scope(scope: Scope) -> String {
    style_scope_name(&scope.to_string())
}

pub(super) fn style_count<T>(value: T) -> String
where
    T: Display,
{
    colorize(&value.to_string(), "1;36")
}

pub(super) fn style_asset_heading<K, S>(kind: K, scope: S, name: &str) -> String
where
    K: Display,
    S: Display,
{
    style_asset_heading_with_colors(kind, scope, name, output_colors_enabled())
}

fn style_asset_heading_with_colors<K, S>(
    kind: K,
    scope: S,
    name: &str,
    colors_enabled: bool,
) -> String
where
    K: Display,
    S: Display,
{
    format!(
        "{} [{}] {}",
        colorize_with_mode(&kind.to_string(), "1;36", colors_enabled),
        style_scope_name_with_colors(&scope.to_string(), colors_enabled),
        colorize_with_mode(name, "1", colors_enabled)
    )
}

fn colorize(text: &str, ansi_code: &str) -> String {
    colorize_with_mode(text, ansi_code, output_colors_enabled())
}

fn colorize_with_mode(text: &str, ansi_code: &str, colors_enabled: bool) -> String {
    if colors_enabled {
        format!("\u{1b}[{ansi_code}m{text}\u{1b}[0m")
    } else {
        text.to_owned()
    }
}

fn colors_enabled(
    is_terminal: bool,
    no_color: bool,
    clicolor_disabled: bool,
    clicolor_force: bool,
    dumb_terminal: bool,
) -> bool {
    if no_color || clicolor_disabled {
        return false;
    }
    if clicolor_force {
        return true;
    }

    is_terminal && !dumb_terminal
}

fn output_colors_enabled() -> bool {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let clicolor_disabled =
        std::env::var_os("CLICOLOR").is_some_and(|value| value.to_string_lossy() == "0");
    let clicolor_force =
        std::env::var_os("CLICOLOR_FORCE").is_some_and(|value| value.to_string_lossy() != "0");
    let dumb_terminal =
        std::env::var_os("TERM").is_some_and(|value| value.to_string_lossy() == "dumb");

    colors_enabled(
        std::io::stdout().is_terminal(),
        no_color,
        clicolor_disabled,
        clicolor_force,
        dumb_terminal,
    )
}

fn format_sub_asset_ownership(ownership: SubAssetOwnership) -> &'static str {
    match ownership {
        SubAssetOwnership::Parent => "parent",
        SubAssetOwnership::Standalone => "standalone",
    }
}

pub(super) fn remove_manifest_asset(
    manifest: &mut Manifest,
    kind: AssetKind,
    name: &str,
    scope: Option<Scope>,
) -> Option<AssetSource> {
    let matches_scope = |source: &AssetSource| {
        let effective_scope = if kind == AssetKind::Plugin {
            effective_plugin_scope(source)
        } else {
            source.scope
        };
        scope
            .map(|requested| effective_scope == requested)
            .unwrap_or(true)
    };

    match kind {
        AssetKind::Plugin => remove_from_sections(
            &mut manifest.plugins,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.plugins,
        ),
        AssetKind::Skill => remove_from_sections(
            &mut manifest.skills,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.skills,
        ),
        AssetKind::Agent => remove_from_sections(
            &mut manifest.agents,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.agents,
        ),
        AssetKind::Mcp => remove_from_sections(
            &mut manifest.mcps,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.mcps,
        ),
        AssetKind::Hook => remove_from_sections(
            &mut manifest.hooks,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.hooks,
        ),
        AssetKind::Workflow => remove_from_sections(
            &mut manifest.workflows,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.workflows,
        ),
        AssetKind::Instruction => remove_from_sections(
            &mut manifest.instructions,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.instructions,
        ),
    }
}

pub(super) fn find_manifest_asset_mut<'a>(
    manifest: &'a mut Manifest,
    kind: AssetKind,
    name: &str,
    scope: Option<Scope>,
) -> Option<&'a mut AssetSource> {
    let matches_scope = |source: &AssetSource| {
        let effective_scope = if kind == AssetKind::Plugin {
            effective_plugin_scope(source)
        } else {
            source.scope
        };
        scope
            .map(|requested| effective_scope == requested)
            .unwrap_or(true)
    };

    match kind {
        AssetKind::Plugin => find_in_sections_mut(
            &mut manifest.plugins,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.plugins,
        ),
        AssetKind::Skill => find_in_sections_mut(
            &mut manifest.skills,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.skills,
        ),
        AssetKind::Agent => find_in_sections_mut(
            &mut manifest.agents,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.agents,
        ),
        AssetKind::Mcp => find_in_sections_mut(
            &mut manifest.mcps,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.mcps,
        ),
        AssetKind::Hook => find_in_sections_mut(
            &mut manifest.hooks,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.hooks,
        ),
        AssetKind::Workflow => find_in_sections_mut(
            &mut manifest.workflows,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.workflows,
        ),
        AssetKind::Instruction => find_in_sections_mut(
            &mut manifest.instructions,
            &mut manifest.groups,
            name,
            matches_scope,
            |group| &mut group.instructions,
        ),
    }
}

fn remove_from_sections<'a, F, G>(
    top_level: &mut indexmap::IndexMap<String, AssetSource>,
    groups: &'a mut indexmap::IndexMap<String, cpm_types::ManifestGroup>,
    name: &str,
    matches_scope: F,
    mut group_section: G,
) -> Option<AssetSource>
where
    F: Fn(&AssetSource) -> bool,
    G: FnMut(&'a mut cpm_types::ManifestGroup) -> &'a mut indexmap::IndexMap<String, AssetSource>,
{
    if top_level.get(name).is_some_and(&matches_scope) {
        return top_level.shift_remove(name);
    }

    for group in groups.values_mut() {
        let section = group_section(group);
        if section.get(name).is_some_and(&matches_scope) {
            return section.shift_remove(name);
        }
    }

    None
}

fn find_in_sections_mut<'a, F, G>(
    top_level: &'a mut indexmap::IndexMap<String, AssetSource>,
    groups: &'a mut indexmap::IndexMap<String, cpm_types::ManifestGroup>,
    name: &str,
    matches_scope: F,
    mut group_section: G,
) -> Option<&'a mut AssetSource>
where
    F: Fn(&AssetSource) -> bool,
    G: FnMut(&'a mut cpm_types::ManifestGroup) -> &'a mut indexmap::IndexMap<String, AssetSource>,
{
    if let Some(source) = top_level.get_mut(name) {
        if matches_scope(source) {
            return Some(source);
        }
    }

    for group in groups.values_mut() {
        let section = group_section(group);
        if let Some(source) = section.get_mut(name) {
            if matches_scope(source) {
                return Some(source);
            }
        }
    }

    None
}

async fn run_scope_transition(
    name: String,
    kind: AssetKind,
    target_scope: Scope,
) -> Result<(), CpmError> {
    let manifest_path = Path::new("cpm.toml");
    let lockfile_path = Path::new("cpm.lock");
    let repo_root = Path::new(".");
    let mut manifest = load_manifest(manifest_path)?;
    let mut delegated_plugin = false;
    let mut normalized_manifest_scope = false;

    let current_scope = {
        let source =
            find_manifest_asset_mut(&mut manifest, kind, &name, None).ok_or_else(|| {
                CpmError::InvalidSource {
                    input: name.clone(),
                    reason: format!(
                        "no {kind} named '{name}' found in {}",
                        manifest_path.display()
                    ),
                }
            })?;
        if kind == AssetKind::Plugin && !plugin_source_is_native(source) {
            delegated_plugin = true;
            if target_scope == Scope::Local {
                return Err(CpmError::InvalidConfig {
                    key: "scope".to_owned(),
                    reason: format!(
                        "delegated plugin '{name}' is installed through `copilot plugin install` and can only use global scope"
                    ),
                });
            }
            let current_scope = effective_plugin_scope(source);
            if source.scope != Scope::Global {
                source.scope = Scope::Global;
                normalized_manifest_scope = true;
            }
            current_scope
        } else {
            if source.scope == target_scope {
                println!(
                    "{} {kind} '{}' is already installed in {} scope",
                    style_success("✓"),
                    colorize(&name, "1"),
                    style_scope(target_scope)
                );
                return Ok(());
            }
            let current_scope = source.scope;
            source.scope = target_scope;
            current_scope
        }
    };

    if delegated_plugin {
        let mut lockfile = load_lockfile(lockfile_path).unwrap_or_default();
        let normalized_lock_scope = normalize_delegated_plugin_lock_entry(&mut lockfile, &name);
        if normalized_manifest_scope || normalized_lock_scope {
            write_manifest(manifest_path, &manifest)?;
            write_lockfile(lockfile_path, &lockfile)?;
            println!(
                "{} normalized delegated plugin '{}' to {} scope",
                style_success("✓"),
                colorize(&name, "1"),
                style_scope(Scope::Global)
            );
        } else {
            println!(
                "{} {kind} '{}' is already installed in {} scope",
                style_success("✓"),
                colorize(&name, "1"),
                style_scope(Scope::Global)
            );
        }
        return Ok(());
    }

    let runtime = load_runtime_config(&manifest)?;
    let reporter = ProgressReporter::auto();
    let client = build_http_client(
        format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        &runtime.settings,
    )?;
    let token = core_auth::resolve_token();
    let previous_asset = load_lockfile(lockfile_path)?
        .all_assets()
        .find(|asset| asset.kind == kind && asset.name == name && asset.scope == current_scope)
        .cloned();

    let lockfile = apply_manifest(
        &manifest,
        &client,
        token.as_deref(),
        ApplyOptions {
            repo_root,
            install: false,
            install_group: None,
            install_scope: None,
            settings: &runtime.settings,
            source_rules: &runtime.source_rules,
            existing_lock: None,
            download_progress: Some(&reporter),
        },
    )
    .await?;

    let resolved_asset =
        find_locked_asset(&lockfile, kind, &name, Some(target_scope)).ok_or_else(|| {
            CpmError::InvalidSource {
                input: name.clone(),
                reason: format!(
                    "resolved {kind} '{name}' was not present in the generated lockfile"
                ),
            }
        })?;

    install_resolved_asset(
        resolved_asset,
        &client,
        token.as_deref(),
        repo_root,
        Some(&reporter),
        &runtime.source_rules,
    )
    .await?;

    write_manifest(manifest_path, &manifest)?;
    write_lockfile(lockfile_path, &lockfile)?;

    if let Some(previous_asset) = &previous_asset {
        remove_asset(previous_asset, repo_root)?;
    }

    println!(
        "{} moved {kind} '{}' from {} to {} scope",
        style_success("✓"),
        colorize(&name, "1"),
        style_scope(current_scope),
        style_scope(target_scope)
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginAction {
    Install,
    Remove,
    Update,
}

impl PluginAction {
    fn operation_kind(self) -> OperationKind {
        match self {
            Self::Install => OperationKind::Install,
            Self::Remove => OperationKind::Remove,
            Self::Update => OperationKind::Update,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PluginOperation {
    action: PluginAction,
    subject: String,
    request: String,
}

impl PluginOperation {
    pub(super) fn install(subject: impl Into<String>, request: impl Into<String>) -> Self {
        Self {
            action: PluginAction::Install,
            subject: subject.into(),
            request: request.into(),
        }
    }

    pub(super) fn remove_with_request(
        subject: impl Into<String>,
        request: impl Into<String>,
    ) -> Self {
        Self {
            action: PluginAction::Remove,
            subject: subject.into(),
            request: request.into(),
        }
    }

    pub(super) fn update_with_request(
        subject: impl Into<String>,
        request: impl Into<String>,
    ) -> Self {
        Self {
            action: PluginAction::Update,
            subject: subject.into(),
            request: request.into(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct PluginOperationSummary {
    pub installed: usize,
    pub removed: usize,
    pub updated: usize,
    pub failed: usize,
}

pub(super) async fn run_plugin_operations(
    operations: Vec<PluginOperation>,
) -> Result<PluginOperationSummary, CpmError> {
    let reporter = ProgressReporter::auto();
    let copilot_bin = std::env::var("CPM_COPILOT_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let delegate = match copilot_bin {
        Some(bin) => PluginDelegate::with_binary(bin),
        None => PluginDelegate::default(),
    };

    let mut summary = PluginOperationSummary::default();
    for operation in operations {
        let mut handle =
            reporter.begin_operation(operation.action.operation_kind(), operation.subject.clone());
        handle.set_status(OperationStatus::Running);
        let outcome = match operation.action {
            PluginAction::Install => delegate.install(&operation.request).await,
            PluginAction::Remove => delegate.uninstall(&operation.request).await,
            PluginAction::Update => delegate.update(&operation.request).await,
        };
        match outcome {
            Ok(()) => {
                handle.set_status(OperationStatus::Succeeded);
                match operation.action {
                    PluginAction::Install => summary.installed += 1,
                    PluginAction::Remove => summary.removed += 1,
                    PluginAction::Update => summary.updated += 1,
                }
            }
            Err(err) => {
                handle.set_status(OperationStatus::Failed);
                return Err(err);
            }
        }
    }

    Ok(summary)
}

pub(super) fn report_skipped_plugin(action: PluginAction, subject: &str) {
    let reporter = ProgressReporter::auto();
    let handle = reporter.begin_operation(action.operation_kind(), subject.to_owned());
    handle.finish(OperationStatus::Skipped);
}

pub(super) fn print_plugin_summary(summary: PluginOperationSummary) {
    println!(
        "{}",
        render_plugin_summary(summary, output_colors_enabled())
    );
}

fn render_plugin_summary(summary: PluginOperationSummary, colors_enabled: bool) -> String {
    format!(
        "{} {} installed{} {} removed{} {} updated{} {} failed",
        colorize_with_mode("Plugins:", "1", colors_enabled),
        colorize_with_mode(&summary.installed.to_string(), "32", colors_enabled),
        colorize_with_mode(" ·", "2", colors_enabled),
        colorize_with_mode(&summary.removed.to_string(), "33", colors_enabled),
        colorize_with_mode(" ·", "2", colors_enabled),
        colorize_with_mode(&summary.updated.to_string(), "1;36", colors_enabled),
        colorize_with_mode(" ·", "2", colors_enabled),
        if summary.failed > 0 {
            colorize_with_mode(&summary.failed.to_string(), "31", colors_enabled)
        } else {
            colorize_with_mode(&summary.failed.to_string(), "2", colors_enabled)
        }
    )
}

pub(super) fn plugin_requested_spec(name: &str, source: &AssetSource) -> String {
    source.url.clone().unwrap_or_else(|| name.to_owned())
}

pub(super) fn plugin_request_is_native(request: &str) -> bool {
    normalize_asset_source(AssetKind::Plugin, request).is_ok()
}

pub(super) fn plugin_source_is_native(source: &AssetSource) -> bool {
    source.path.is_some() || source.url.as_deref().is_some_and(plugin_request_is_native)
}

pub(super) fn plugin_asset_is_delegated(asset: &ResolvedAsset) -> bool {
    asset.kind == AssetKind::Plugin && !plugin_source_is_native(&asset.source)
}

pub(super) fn effective_plugin_scope(source: &AssetSource) -> Scope {
    if plugin_source_is_native(source) {
        source.scope
    } else {
        Scope::Global
    }
}

pub(super) fn effective_asset_scope(asset: &ResolvedAsset) -> Scope {
    if plugin_asset_is_delegated(asset) {
        Scope::Global
    } else {
        asset.scope
    }
}

pub(super) fn discovered_plugin_request(
    installed_plugins: &[InstalledPlugin],
    name: &str,
) -> String {
    find_installed_plugin_by_name(installed_plugins, name)
        .and_then(installed_plugin_request)
        .unwrap_or_else(|| name.to_owned())
}

pub(super) fn derive_plugin_name(request: &str) -> String {
    request
        .trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(request)
        .split('@')
        .next()
        .unwrap_or(request)
        .to_owned()
}

pub(super) fn strip_delegated_plugins_from_manifest(mut manifest: Manifest) -> Manifest {
    manifest
        .plugins
        .retain(|_, source| plugin_source_is_native(source));
    for group in manifest.groups.values_mut() {
        group
            .plugins
            .retain(|_, source| plugin_source_is_native(source));
    }
    manifest
}

pub(super) fn merge_delegated_plugin_lock_entries(
    lockfile: &mut Lockfile,
    delegated_entries: Vec<ResolvedAsset>,
) {
    lockfile
        .plugins
        .retain(|asset| !plugin_asset_is_delegated(asset));
    lockfile.plugins.extend(delegated_entries);
}

pub(super) fn collect_plugin_lock_entries(
    manifest: &Manifest,
    existing_lock: &Lockfile,
    installed_plugins: &[InstalledPlugin],
    refresh_keys: Option<&HashSet<(String, Scope)>>,
    require_refreshed: bool,
) -> Result<Vec<ResolvedAsset>, CpmError> {
    let mut plugins = Vec::new();
    for (name, source) in manifest.effective_section(AssetKind::Plugin) {
        let key = (name.clone(), effective_plugin_scope(&source));
        let refresh = refresh_keys.map(|keys| keys.contains(&key)).unwrap_or(true);

        if refresh {
            if let Some(installed) = find_installed_plugin_by_name(installed_plugins, &name) {
                plugins.push(build_locked_plugin_asset(&name, source, installed)?);
                continue;
            }

            if !require_refreshed {
                if let Some(existing) =
                    find_locked_asset(existing_lock, AssetKind::Plugin, &name, Some(key.1)).cloned()
                {
                    plugins.push(existing);
                    continue;
                }
            }

            return Err(CpmError::InvalidSource {
                input: name.clone(),
                reason: format!("plugin '{name}' is not installed according to Copilot discovery"),
            });
        }

        if let Some(existing) =
            find_locked_asset(existing_lock, AssetKind::Plugin, &name, Some(key.1)).cloned()
        {
            plugins.push(existing);
            continue;
        }

        if let Some(installed) = find_installed_plugin_by_name(installed_plugins, &name) {
            plugins.push(build_locked_plugin_asset(&name, source, installed)?);
        }
    }

    Ok(plugins)
}

pub(super) fn build_locked_plugin_asset(
    name: &str,
    source: AssetSource,
    installed: &InstalledPlugin,
) -> Result<ResolvedAsset, CpmError> {
    let mut normalized_source = source;
    let normalized_scope = effective_plugin_scope(&normalized_source);
    normalized_source.scope = normalized_scope;
    let plugin_json_hash = hash_plugin_manifest(installed)?;
    let fallback_hash_input = format!(
        "{}|{}|{}|{}",
        name,
        installed.revision.as_deref().unwrap_or_default(),
        installed.version.as_deref().unwrap_or_default(),
        installed.source.as_deref().unwrap_or_default()
    );
    let hash = plugin_json_hash
        .clone()
        .unwrap_or_else(|| sha256_hex(fallback_hash_input.as_bytes()));
    let plugin_meta = PluginMeta {
        registry: installed.registry.clone(),
        plugin_version: installed.version.clone(),
        source_url: installed.source.clone(),
        plugin_json_hash,
    };

    Ok(ResolvedAsset {
        name: name.to_owned(),
        kind: AssetKind::Plugin,
        source: normalized_source,
        resolved_rev: installed
            .revision
            .clone()
            .or_else(|| installed.version.clone())
            .unwrap_or_else(|| hash.clone()),
        resolved_date: Utc::now(),
        hash,
        scope: normalized_scope,
        ownership: AssetOwnership::Upstream,
        files: Vec::new(),
        executable: Vec::new(),
        file_hashes: Default::default(),
        git: None,
        sub_assets: Vec::new(),
        license: None,
        bin_path: None,
        compiled_path: None,
        plugin_meta: (!plugin_meta.is_empty()).then_some(plugin_meta),
    })
}

pub(super) fn upsert_plugin_lock_entry(lockfile: &mut Lockfile, asset: ResolvedAsset) {
    if plugin_asset_is_delegated(&asset) {
        let mut replaced = false;
        lockfile.plugins.retain_mut(|existing| {
            let matches_name = existing.name == asset.name && plugin_asset_is_delegated(existing);
            if !matches_name {
                return true;
            }
            if replaced {
                return false;
            }
            *existing = asset.clone();
            replaced = true;
            true
        });
        if !replaced {
            lockfile.plugins.push(asset);
        }
        return;
    }

    if let Some(existing) = lockfile
        .plugins
        .iter_mut()
        .find(|existing| existing.name == asset.name && existing.scope == asset.scope)
    {
        *existing = asset;
    } else {
        lockfile.plugins.push(asset);
    }
}

pub(super) fn normalize_delegated_plugin_lock_entry(lockfile: &mut Lockfile, name: &str) -> bool {
    let mut changed = false;
    for asset in lockfile
        .plugins
        .iter_mut()
        .filter(|asset| asset.name == name && plugin_asset_is_delegated(asset))
    {
        if asset.scope != Scope::Global {
            asset.scope = Scope::Global;
            changed = true;
        }
        if asset.source.scope != Scope::Global {
            asset.source.scope = Scope::Global;
            changed = true;
        }
    }
    changed
}

pub(super) fn remove_plugin_lock_entry(lockfile: &mut Lockfile, name: &str, scope: Scope) {
    lockfile
        .plugins
        .retain(|asset| !(asset.name == name && asset.scope == scope));
}

fn hash_plugin_manifest(plugin: &InstalledPlugin) -> Result<Option<String>, CpmError> {
    hash_installed_plugin_manifest(plugin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::TempDir;

    #[test]
    fn resolve_required_kind_accepts_single_flag() {
        assert_eq!(
            resolve_required_kind(false, true, false, false, false, false, false).expect("kind"),
            AssetKind::Skill
        );
    }

    #[test]
    fn resolve_required_kind_rejects_missing_flag() {
        let err = resolve_required_kind(false, false, false, false, false, false, false)
            .expect_err("missing flag");
        assert!(matches!(err, CpmError::InvalidConfig { .. }));
    }

    #[test]
    fn derive_plugin_name_strips_registry_suffix() {
        assert_eq!(derive_plugin_name("pptx@awesome-copilot"), "pptx");
        assert_eq!(derive_plugin_name("nested/pptx@registry"), "pptx");
    }

    #[test]
    fn build_locked_plugin_asset_captures_plugin_meta_and_manifest_hash() {
        let dir = TempDir::new().expect("tempdir");
        let plugin_root = dir.path().join("pptx");
        let plugin_json = plugin_root.join(".github/plugin/plugin.json");
        std::fs::create_dir_all(plugin_json.parent().expect("parent")).expect("mkdir");
        std::fs::write(&plugin_json, br#"{"name":"pptx"}"#).expect("write plugin json");

        let asset = build_locked_plugin_asset(
            "pptx",
            AssetSource {
                url: Some("pptx@awesome-copilot".into()),
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Local,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            &InstalledPlugin {
                name: Some("pptx".into()),
                version: Some("1.2.3".into()),
                revision: Some("deadbeef".into()),
                source: Some("https://example.test/pptx".into()),
                registry: Some("awesome-copilot".into()),
                description: None,
                path: Some(Utf8PathBuf::from_path_buf(plugin_root).expect("utf8 path")),
                enabled: Some(true),
                installed_at: None,
                extra: Default::default(),
            },
        )
        .expect("locked asset");

        assert_eq!(asset.kind, AssetKind::Plugin);
        assert_eq!(asset.resolved_rev, "deadbeef");
        assert_eq!(
            asset
                .plugin_meta
                .as_ref()
                .and_then(|meta| meta.registry.as_deref()),
            Some("awesome-copilot")
        );
        assert_eq!(
            asset
                .plugin_meta
                .as_ref()
                .and_then(|meta| meta.plugin_json_hash.clone()),
            Some(asset.hash.clone())
        );
    }

    #[test]
    fn style_asset_heading_keeps_plain_shape_without_color() {
        assert_eq!(
            style_asset_heading_with_colors(AssetKind::Skill, Scope::Local, "tracked", false),
            "skill [local] tracked"
        );
    }

    #[test]
    fn plugin_summary_plain_text_uses_new_human_readable_shape() {
        let summary = PluginOperationSummary {
            installed: 1,
            removed: 0,
            updated: 2,
            failed: 0,
        };
        let rendered = render_plugin_summary(summary, false);

        assert_eq!(
            rendered,
            "Plugins: 1 installed · 0 removed · 2 updated · 0 failed"
        );
    }

    #[test]
    fn colors_enabled_respects_priority_rules() {
        assert!(!colors_enabled(true, true, false, true, false));
        assert!(!colors_enabled(true, false, true, true, false));
        assert!(colors_enabled(false, false, false, true, false));
        assert!(!colors_enabled(true, false, false, false, true));
        assert!(colors_enabled(true, false, false, false, false));
        assert!(!colors_enabled(false, false, false, false, false));
    }
}
