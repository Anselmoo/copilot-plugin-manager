//! Integration tests for the `cpm` CLI binary.
//!
//! These tests invoke the compiled `cpm` binary via `std::process::Command`
//! (or via `cargo run`) to verify end-to-end command behaviour.

use std::{path::Path, process::Command};

use camino::Utf8PathBuf;
use cpm_core::project::{
    load_global_lockfile_from, load_lockfile, write_global_lockfile_to, write_lockfile,
    write_manifest,
};
use cpm_types::{
    AssetKind, AssetOwnership, AssetSource, GlobalClaim, GlobalLockfile, Lockfile, Manifest,
    ManifestGroup, McpTransport, PluginMeta, ResolvedAsset, Scope, SubAsset, SubAssetOwnership,
};
use serde_json::Value;

fn cpm_bin() -> Command {
    // Use the already-compiled debug binary when running `cargo test`.
    let bin = env!("CARGO_BIN_EXE_cpm");
    let mut cmd = Command::new(bin);
    cmd.env("NO_COLOR", "1");
    cmd
}

fn normalized_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn write_global_skill_manifest(repo_root: &Path, asset_path: &Path) {
    let manifest = format!(
        "[skills]\nshared = {{ path = \"{}\", scope = \"global\" }}\n",
        normalized_path_string(asset_path)
    );
    std::fs::write(repo_root.join("cpm.toml"), manifest).expect("write manifest");
}

fn make_global_claim(claimed_by: &Path, hash: &str) -> GlobalClaim {
    GlobalClaim::new(
        Utf8PathBuf::from_path_buf(claimed_by.to_path_buf()).expect("utf8 claimed_by"),
        ResolvedAsset {
            name: "shared".into(),
            kind: AssetKind::Skill,
            source: AssetSource {
                url: Some("https://example.com/shared-skill".into()),
                rev: None,
                path: None,
                group: "default".into(),
                scope: Scope::Global,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            resolved_date: chrono::Utc::now(),
            hash: hash.into(),
            scope: Scope::Global,
            ownership: AssetOwnership::Upstream,
            files: vec![Utf8PathBuf::from("shared/SKILL.md").into()],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        },
    )
}

fn make_source(path: &str) -> AssetSource {
    AssetSource {
        url: Some("https://example.com/partners".to_owned()),
        rev: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
        path: Some(Utf8PathBuf::from(path)),
        group: "default".to_owned(),
        scope: Scope::Local,
        transport: None,
        env: vec![],
        args: vec![],
        engine: None,
    }
}

fn make_resolved(name: &str) -> ResolvedAsset {
    ResolvedAsset {
        name: name.to_owned(),
        kind: AssetKind::Plugin,
        source: make_source("plugins/partners"),
        resolved_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        resolved_date: chrono::Utc::now(),
        hash: "sha256:partners".to_owned(),
        scope: Scope::Local,
        ownership: AssetOwnership::Upstream,
        files: vec![],
        executable: vec![],
        file_hashes: Default::default(),
        git: None,
        sub_assets: vec![
            SubAsset {
                name: "terraform".to_owned(),
                kind: AssetKind::Agent,
                path: Utf8PathBuf::from("partners/agents/terraform.md"),
                ownership: SubAssetOwnership::Parent,
            },
            SubAsset {
                name: "prompt-lib".to_owned(),
                kind: AssetKind::Skill,
                path: Utf8PathBuf::from("partners/skills/prompt-lib"),
                ownership: SubAssetOwnership::Parent,
            },
        ],
        license: None,
        bin_path: None,
        compiled_path: None,
        plugin_meta: None,
    }
}

fn make_instruction_source(path: &str) -> AssetSource {
    AssetSource {
        url: Some("https://example.com/instructions/shell.instructions.md".to_owned()),
        rev: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned()),
        path: Some(Utf8PathBuf::from(path)),
        group: "default".to_owned(),
        scope: Scope::Local,
        transport: None,
        env: vec![],
        args: vec![],
        engine: None,
    }
}

fn make_instruction_resolved(name: &str) -> ResolvedAsset {
    ResolvedAsset {
        name: name.to_owned(),
        kind: AssetKind::Instruction,
        source: make_instruction_source("instructions/shell.instructions.md"),
        resolved_rev: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        resolved_date: chrono::Utc::now(),
        hash: "sha256:shell".to_owned(),
        scope: Scope::Local,
        ownership: AssetOwnership::Upstream,
        files: vec![Utf8PathBuf::from("shell.instructions.md").into()],
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

fn write_reporting_fixture(dir: &tempfile::TempDir) {
    let mut manifest = Manifest::default();
    manifest.plugins.insert(
        "partners".to_owned(),
        AssetSource {
            url: Some("https://example.com/partners".to_owned()),
            rev: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
            path: Some(Utf8PathBuf::from("plugins/partners")),
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        },
    );
    write_manifest(&dir.path().join("cpm.toml"), &manifest).expect("write manifest");

    let mut lockfile = Lockfile::new();
    lockfile.plugins.push(make_resolved("partners"));
    write_lockfile(&dir.path().join("cpm.lock"), &lockfile).expect("write lockfile");
}

fn write_plugin_manifest(repo_root: &Path, name: &str, url: &str) {
    let mut manifest = Manifest::default();
    manifest.plugins.insert(
        name.to_owned(),
        AssetSource {
            url: Some(url.to_owned()),
            rev: None,
            path: None,
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        },
    );
    write_manifest(&repo_root.join("cpm.toml"), &manifest).expect("write plugin manifest");
}

fn write_plugin_path_manifest(repo_root: &Path, name: &str, path: &str) {
    let mut manifest = Manifest::default();
    manifest.plugins.insert(
        name.to_owned(),
        AssetSource {
            url: None,
            rev: None,
            path: Some(Utf8PathBuf::from(path)),
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        },
    );
    write_manifest(&repo_root.join("cpm.toml"), &manifest).expect("write plugin path manifest");
}

fn seed_plugin_lock(repo_root: &Path, name: &str, url: &str, version: &str, revision: &str) {
    let mut lockfile = Lockfile::new();
    lockfile.plugins.push(ResolvedAsset {
        name: name.to_owned(),
        kind: AssetKind::Plugin,
        source: AssetSource {
            url: Some(url.to_owned()),
            rev: None,
            path: None,
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        },
        resolved_rev: revision.to_owned(),
        resolved_date: chrono::Utc::now(),
        hash: format!("sha256:{revision}"),
        scope: Scope::Local,
        ownership: AssetOwnership::Upstream,
        files: vec![],
        executable: vec![],
        file_hashes: Default::default(),
        git: None,
        sub_assets: vec![],
        license: None,
        bin_path: None,
        compiled_path: None,
        plugin_meta: Some(PluginMeta {
            registry: Some("awesome-copilot".into()),
            plugin_version: Some(version.into()),
            source_url: Some(format!("https://example.test/{name}")),
            plugin_json_hash: Some(format!("sha256:{revision}")),
        }),
    });
    write_lockfile(&repo_root.join("cpm.lock"), &lockfile).expect("write plugin lock");
}

fn write_fake_copilot(dir: &tempfile::TempDir) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        let script_path = dir.path().join("fake-copilot");
        let script = r#"#!/bin/sh
set -eu
log_file="${CPM_TEST_LOG:?}"
home_dir="${HOME:?}"
copilot_dir="$home_dir/.copilot"
legacy_plugin_dir="$copilot_dir/plugins"
installed_plugins_dir="$copilot_dir/installed-plugins"
mkdir -p "$copilot_dir" "$legacy_plugin_dir" "$installed_plugins_dir"
printf '%s %s %s\n' "$1" "$2" "$3" >> "$log_file"
op="$2"
request="$3"
name="${request%@*}"
if [ "$name" != "$request" ]; then
  registry="${request#*@}"
else
  registry="awesome-copilot"
fi
plugin_root="$installed_plugins_dir/$registry/$name"
marker_path="$legacy_plugin_dir/$name.installed"
index_path="$copilot_dir/plugin-index.json"
write_entry() {
  version="$1"
  revision="$2"
  marker="$3"
  mkdir -p "$plugin_root/.github/plugin"
  : > "$marker_path"
  printf '{"name":"%s","marker":"%s"}\n' "$name" "$marker" > "$plugin_root/.github/plugin/plugin.json"
  cat > "$index_path" <<EOF
{"plugins":[{"name":"$name","version":"$version","revision":"$revision","source_url":"https://example.test/$name","registry":"$registry","path":"$plugin_root","enabled":true}]}
EOF
}
case "$op" in
  install)
    write_entry "1.0.0" "rev-install" "install"
    ;;
  update)
    write_entry "2.0.0" "rev-update" "update"
    ;;
  uninstall)
    rm -rf "$plugin_root"
    rm -f "$marker_path"
    cat > "$index_path" <<EOF
{"plugins":[]}
EOF
    ;;
  *)
    echo "unsupported operation: $op" >&2
    exit 1
    ;;
esac
"#;
        std::fs::write(&script_path, script).expect("write fake copilot");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake copilot");
        script_path
    }

    #[cfg(windows)]
    {
        let script_path = dir.path().join("fake-copilot.cmd");
        let script = r#"@echo off
setlocal enabledelayedexpansion
set "log_file=%CPM_TEST_LOG%"
if not defined log_file (
    echo CPM_TEST_LOG environment variable not set 1>&2
    exit /b 1
)
set "home_dir=%USERPROFILE%"
if not defined home_dir (
    echo USERPROFILE environment variable not set 1>&2
    exit /b 1
)
set "copilot_dir=%home_dir%\.copilot"
set "legacy_plugin_dir=%copilot_dir%\plugins"
set "installed_plugins_dir=%copilot_dir%\installed-plugins"

if not exist "%copilot_dir%" mkdir "%copilot_dir%"
if not exist "%legacy_plugin_dir%" mkdir "%legacy_plugin_dir%"
if not exist "%installed_plugins_dir%" mkdir "%installed_plugins_dir%"

echo %1 %2 %3 >> "%log_file%"

set "op=%2"
set "request=%3"
set "name=%request:@=^>%"
if not "!name!"=="!request!" (
    for /f "tokens=1 delims=@" %%a in ("!request!") do set "name=%%a"
    for /f "tokens=2 delims=@" %%a in ("!request!") do set "registry=%%a"
) else (
    set "registry=awesome-copilot"
)
set "plugin_root=%installed_plugins_dir%\!registry!\!name!"
set "marker_path=%legacy_plugin_dir%\!name!.installed"
set "index_path=%copilot_dir%\plugin-index.json"

if "%op%"=="install" (
    call :write_entry "1.0.0" "rev-install" "install"
) else if "%op%"=="update" (
    call :write_entry "2.0.0" "rev-update" "update"
) else if "%op%"=="uninstall" (
    if exist "%plugin_root%" rmdir /s /q "%plugin_root%"
    if exist "%marker_path%" del "%marker_path%"
    (
        echo {"plugins":[]}
    ) > "%index_path%"
) else (
    echo unsupported operation: %op% 1>&2
    exit /b 1
)
exit /b 0

:write_entry
set "version=%~1"
set "revision=%~2"
set "marker=%~3"
set "escaped_plugin_root=!plugin_root:\=\\!"
if not exist "%plugin_root%\.github\plugin" mkdir "%plugin_root%\.github\plugin"
type nul > "%marker_path%"
(
    echo {"name":"!name!","marker":"!marker!"}
) > "%plugin_root%\.github\plugin\plugin.json"
(
    echo {"plugins":[{"name":"!name!","version":"!version!","revision":"!revision!","source_url":"https://example.test/!name!","registry":"!registry!","path":"!escaped_plugin_root!","enabled":true}]}
) > "%index_path%"
exit /b 0
"#;
        std::fs::write(&script_path, script).expect("write fake copilot");
        script_path
    }
}

fn fake_copilot_env(
    fake_home: &tempfile::TempDir,
    fake_copilot_dir: &tempfile::TempDir,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let script_path = write_fake_copilot(fake_copilot_dir);
    let log_path = fake_home.path().join("copilot.log");
    (script_path, log_path)
}

fn write_not_installed_copilot(dir: &tempfile::TempDir) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        let script_path = dir.path().join("fake-copilot-not-installed");
        let script = r#"#!/bin/sh
set -eu
log_file="${CPM_TEST_LOG:?}"
printf '%s %s %s\n' "$1" "$2" "$3" >> "$log_file"
if [ "$2" = "uninstall" ]; then
  echo "Failed to uninstall plugin: Plugin \"$3\" is not installed" >&2
  exit 1
fi
echo "unsupported operation: $2" >&2
exit 1
"#;
        std::fs::write(&script_path, script).expect("write fake copilot");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake copilot");
        script_path
    }

    #[cfg(windows)]
    {
        let script_path = dir.path().join("fake-copilot-not-installed.cmd");
        let script = r#"@echo off
setlocal enabledelayedexpansion
set "log_file=%CPM_TEST_LOG%"
if not defined log_file (
    echo CPM_TEST_LOG environment variable not set 1>&2
    exit /b 1
)
echo %1 %2 %3 >> "%log_file%"
if "%2"=="uninstall" (
    echo Failed to uninstall plugin: Plugin "%3" is not installed 1>&2
    exit /b 1
)
echo unsupported operation: %2 1>&2
exit /b 1
"#;
        std::fs::write(&script_path, script).expect("write fake copilot");
        script_path
    }
}

// ── --help / --version ────────────────────────────────────────────────────────

#[test]
fn help_exits_zero() {
    let output = cpm_bin().arg("--help").output().expect("run cpm --help");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cpm"), "expected 'cpm' in help output");
}

#[test]
fn help_groups_commands_in_logical_order() {
    let output = cpm_bin().arg("--help").output().expect("run cpm --help");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for heading in [
        "Getting started:",
        "Manage assets:",
        "Inspect & diagnose:",
        "Maintenance:",
    ] {
        assert!(
            stdout.contains(heading),
            "expected help output to include heading {heading:?}\n{stdout}"
        );
    }

    let init_index = stdout.find("init").expect("init listed");
    let add_index = stdout.find("add").expect("add listed");
    let sync_index = stdout.find("sync").expect("sync listed");
    let overview_index = stdout.find("overview").expect("overview listed");
    let cache_index = stdout.find("cache").expect("cache listed");

    assert!(
        init_index < add_index,
        "init should appear before add\n{stdout}"
    );
    assert!(
        add_index < sync_index,
        "add should appear before sync\n{stdout}"
    );
    assert!(
        sync_index < overview_index,
        "sync should appear before overview\n{stdout}"
    );
    assert!(
        overview_index < cache_index,
        "overview should appear before cache\n{stdout}"
    );
}

#[test]
fn version_exits_zero() {
    let output = cpm_bin()
        .arg("--version")
        .output()
        .expect("run cpm --version");
    assert!(output.status.success());
}

#[test]
fn subcommand_help_exits_zero() {
    for subcmd in &[
        "init", "add", "sync", "remove", "update", "lock", "list", "show", "doctor", "status",
        "tree", "overview", "run", "reset", "cache", "auth", "scope",
    ] {
        let output = cpm_bin()
            .args([subcmd, "--help"])
            .output()
            .unwrap_or_else(|_| panic!("run cpm {subcmd} --help"));
        assert!(
            output.status.success(),
            "cpm {subcmd} --help failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// ── cpm init ──────────────────────────────────────────────────────────────────

#[test]
fn init_creates_manifest_and_lockfile() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let output = cpm_bin()
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm init");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.path().join("cpm.toml").exists(),
        "cpm.toml should exist"
    );
    assert!(
        dir.path().join("cpm.lock").exists(),
        "cpm.lock should exist"
    );
}

#[test]
fn init_with_explicit_name() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let output = cpm_bin()
        .args(["init", "--name", "my-project"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm init --name my-project");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = std::fs::read_to_string(dir.path().join("cpm.toml")).expect("read cpm.toml");
    assert!(
        content.contains("my-project"),
        "manifest should contain the project name"
    );
}

#[test]
fn init_prints_first_class_next_steps() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let output = cpm_bin()
        .args(["init", "--name", "starter"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm init --name starter");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "cpm add <url> --skill",
        "cpm add <url> --plugin",
        "cpm add <url> --agent",
        "cpm add <url> --mcp",
        "cpm add <url> --hook",
        "cpm add <url> --workflow",
        "cpm sync",
    ] {
        assert!(
            stdout.contains(expected),
            "expected init output to mention {expected:?}\n{stdout}"
        );
    }
}

#[test]
fn init_does_not_overwrite_without_force() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join("cpm.toml"),
        "[package]\nname = 'original'\n",
    )
    .expect("write");

    let output = cpm_bin()
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm init");

    assert!(output.status.success());
    let content = std::fs::read_to_string(dir.path().join("cpm.toml")).expect("read");
    assert!(
        content.contains("original"),
        "existing manifest must not be overwritten"
    );
}

#[test]
fn lock_check_without_lockfile_reports_missing_lockfile() {
    let dir = tempfile::TempDir::new().expect("tempdir");

    let init_output = cpm_bin()
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm init");
    assert!(
        init_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init_output.stderr)
    );

    std::fs::remove_file(dir.path().join("cpm.lock")).expect("remove lockfile");

    let output = cpm_bin()
        .args(["lock", "--check"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm lock --check");

    assert!(
        !output.status.success(),
        "lock --check should fail when cpm.lock is missing"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "expected missing-lockfile error\n{stderr}"
    );
}

// ── cpm add (local path) ──────────────────────────────────────────────────────

#[test]
fn add_local_skill_creates_manifest_and_lock() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let skill_dir = dir.path().join("skills/my-skill");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# My Skill\n").expect("write SKILL.md");

    let output = cpm_bin()
        .args(["add", skill_dir.to_str().expect("utf8"), "--skill"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm add");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("cpm.toml").exists());
    assert!(dir.path().join("cpm.lock").exists());

    let installed = dir.path().join(".github/skills/my-skill/SKILL.md");
    assert!(installed.exists(), "installed skill file should exist");
}

#[test]
fn add_local_skill_writes_canonical_toml() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let skill_dir = dir.path().join("skills/canonical-skill");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Canonical\n").expect("write");

    cpm_bin()
        .args(["add", skill_dir.to_str().expect("utf8"), "--skill"])
        .current_dir(dir.path())
        .output()
        .expect("run cpm add");

    let toml_text = std::fs::read_to_string(dir.path().join("cpm.toml")).expect("read toml");
    assert!(
        toml_text.contains("[skills]"),
        "should use flat [skills] section"
    );
    assert!(
        !toml_text.contains("[skills.canonical-skill]"),
        "should not use nested table form"
    );
}

#[test]
fn add_local_instruction_normalizes_filename_and_uses_group_section() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let instruction_file = dir.path().join("shell.md");
    std::fs::write(&instruction_file, "# Shell\n").expect("write");

    let output = cpm_bin()
        .args([
            "add",
            instruction_file.to_str().expect("utf8"),
            "--instruction",
            "--group",
            "dev",
        ])
        .current_dir(dir.path())
        .output()
        .expect("run cpm add");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let installed = dir
        .path()
        .join(".github/instructions/shell.instructions.md");
    assert!(
        installed.exists(),
        "installed instruction file should exist"
    );

    let toml_text = std::fs::read_to_string(dir.path().join("cpm.toml")).expect("read toml");
    assert!(
        toml_text.contains("[groups.dev.instructions]"),
        "instruction should be stored in the requested dev group"
    );
    assert!(
        toml_text.contains("shell ="),
        "instruction entry should be present in the manifest"
    );

    let lockfile = load_lockfile(&dir.path().join("cpm.lock")).expect("load lockfile");
    assert_eq!(lockfile.instructions.len(), 1);
    assert_eq!(lockfile.instructions[0].name, "shell");
    assert_eq!(
        lockfile.instructions[0].files[0].path,
        Utf8PathBuf::from("shell.instructions.md")
    );
}

#[test]
fn requested_dev_group_examples_round_trip_in_manifest() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let mut manifest = Manifest::default();
    let mut group = ManifestGroup::default();

    let make_source = |url: &str, scope: Scope| AssetSource {
        url: Some(url.to_owned()),
        rev: None,
        path: None,
        group: "dev".to_owned(),
        scope,
        transport: None,
        env: vec![],
        args: vec![],
        engine: None,
    };

    group.plugins.insert(
        "code-review".to_owned(),
        make_source(
            "https://github.com/anthropics/claude-plugins-official/tree/main/plugins/code-review",
            Scope::Global,
        ),
    );
    group.skills.insert(
        "theme-factory".to_owned(),
        make_source(
            "https://github.com/anthropics/skills/tree/main/skills/theme-factory",
            Scope::Global,
        ),
    );
    group.mcps.insert(
        "mcp-ai-agent-guidelines".to_owned(),
        AssetSource {
            url: None,
            rev: None,
            path: None,
            group: "dev".to_owned(),
            scope: Scope::Global,
            transport: Some(McpTransport::Npx {
                package: "mcp-ai-agent-guidelines".to_owned(),
                entrypoint: None,
                args: vec![],
            }),
            env: vec![],
            args: vec![],
            engine: None,
        },
    );
    group.mcps.insert(
        "mcp-zen-of-languages".to_owned(),
        AssetSource {
            url: None,
            rev: None,
            path: None,
            group: "dev".to_owned(),
            scope: Scope::Global,
            transport: Some(McpTransport::Uvx {
                package: "mcp-zen-of-languages".to_owned(),
                entrypoint: None,
                args: vec![],
            }),
            env: vec![],
            args: vec![],
            engine: None,
        },
    );
    group.agents.insert(
        "gem-researcher".to_owned(),
        make_source(
            "https://github.com/github/awesome-copilot/blob/main/agents/gem-researcher.agent.md",
            Scope::Local,
        ),
    );
    group.hooks.insert(
        "secrets-scanner".to_owned(),
        make_source(
            "https://github.com/github/awesome-copilot/tree/main/hooks/secrets-scanner",
            Scope::Local,
        ),
    );
    group.instructions.insert(
        "shell".to_owned(),
        make_source(
            "https://github.com/github/awesome-copilot/blob/main/instructions/shell.instructions.md",
            Scope::Local,
        ),
    );
    manifest.groups.insert("dev".to_owned(), group);

    let manifest_path = dir.path().join("cpm.toml");
    write_manifest(&manifest_path, &manifest).expect("write manifest");
    let loaded = cpm_core::project::load_manifest(&manifest_path).expect("load manifest");
    let dev = loaded.groups.get("dev").expect("dev group");

    assert_eq!(
        dev.plugins["code-review"].scope,
        Scope::Global,
        "plugin example should stay global"
    );
    assert_eq!(dev.skills["theme-factory"].scope, Scope::Global);
    assert_eq!(
        dev.agents["gem-researcher"].url.as_deref(),
        Some("https://github.com/github/awesome-copilot/blob/main/agents/gem-researcher.agent.md")
    );
    assert_eq!(
        dev.instructions["shell"].url.as_deref(),
        Some("https://github.com/github/awesome-copilot/blob/main/instructions/shell.instructions.md")
    );
    assert!(matches!(
        dev.mcps["mcp-ai-agent-guidelines"].transport.as_ref(),
        Some(McpTransport::Npx { package, .. }) if package == "mcp-ai-agent-guidelines"
    ));
    assert!(matches!(
        dev.mcps["mcp-zen-of-languages"].transport.as_ref(),
        Some(McpTransport::Uvx { package, .. }) if package == "mcp-zen-of-languages"
    ));
}

// ── cpm remove ────────────────────────────────────────────────────────────────

#[test]
fn remove_skill_updates_manifest() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let skill_dir = dir.path().join("skills/to-remove");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# To Remove\n").expect("write");

    cpm_bin()
        .args(["add", skill_dir.to_str().expect("utf8"), "--skill"])
        .current_dir(dir.path())
        .output()
        .expect("add");

    let output = cpm_bin()
        .args(["remove", "to-remove", "--skill"])
        .current_dir(dir.path())
        .output()
        .expect("remove");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let toml_text = std::fs::read_to_string(dir.path().join("cpm.toml")).expect("read toml");
    assert!(
        !toml_text.contains("to-remove"),
        "removed skill should not be in manifest"
    );
}

#[test]
fn add_plugin_delegates_and_writes_lock_metadata() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    let output = cpm_bin()
        .args(["add", "pptx@awesome-copilot", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm add plugin");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("Plugins: 1 installed · 0 removed · 0 updated · 0 failed"));
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(log.contains("plugin install pptx@awesome-copilot"));

    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert_eq!(lock.plugins.len(), 1);
    assert_eq!(lock.plugins[0].name, "pptx");
    assert_eq!(lock.plugins[0].resolved_rev, "rev-install");
    assert_eq!(lock.plugins[0].scope, Scope::Global);
    assert_eq!(lock.plugins[0].source.scope, Scope::Global);
    let meta = lock.plugins[0].plugin_meta.as_ref().expect("plugin meta");
    assert_eq!(meta.registry.as_deref(), Some("awesome-copilot"));
    assert_eq!(meta.plugin_version.as_deref(), Some("1.0.0"));
    assert!(meta
        .plugin_json_hash
        .as_deref()
        .unwrap_or_default()
        .starts_with("sha256:"));
    let manifest =
        cpm_core::project::load_manifest(&repo.path().join("cpm.toml")).expect("load manifest");
    assert_eq!(
        manifest.plugins.get("pptx").expect("plugin source").scope,
        Scope::Global
    );
}

#[test]
fn add_plugin_tree_source_installs_natively_without_delegate() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let plugin_dir = repo.path().join("plugins/native-bundle");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir plugin");
    std::fs::create_dir_all(plugin_dir.join("skills/pdf")).expect("mkdir skill");
    std::fs::write(plugin_dir.join("README.md"), "# Native Bundle\n").expect("write readme");
    std::fs::write(
        plugin_dir.join(".github/plugin/plugin.json"),
        r#"{"name":"native-bundle","version":"1.0.0"}"#,
    )
    .expect("write plugin json");
    std::fs::write(plugin_dir.join("skills/pdf/SKILL.md"), "# PDF\n").expect("write skill");

    let output = cpm_bin()
        .args([
            "add",
            plugin_dir.to_str().expect("utf8 plugin dir"),
            "--plugin",
        ])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm add native plugin");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.trim().is_empty(),
        "native plugin add should not delegate: {log}"
    );
    assert!(
        repo.path()
            .join(".github/plugins/native-bundle/README.md")
            .exists(),
        "native plugin files should be installed into the repo"
    );

    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert_eq!(lock.plugins.len(), 1);
    assert!(
        !lock.plugins[0].files.is_empty(),
        "native plugin should track files"
    );
    assert!(lock.plugins[0].plugin_meta.is_none());
    assert_eq!(lock.plugins[0].sub_assets.len(), 1);
}

#[test]
fn remove_plugin_delegates_and_clears_lock_entry() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    write_plugin_manifest(repo.path(), "pptx", "pptx@awesome-copilot");
    seed_plugin_lock(
        repo.path(),
        "pptx",
        "pptx@awesome-copilot",
        "1.0.0",
        "rev-install",
    );
    cpm_bin()
        .args(["add", "pptx@awesome-copilot", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("seed plugin install");
    std::fs::write(&log_path, "").expect("reset log");

    let output = cpm_bin()
        .args(["remove", "pptx", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm remove plugin");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = std::fs::read_to_string(&log_path).expect("read log");
    assert!(log.contains("plugin uninstall pptx@awesome-copilot"));
    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert!(
        lock.plugins.is_empty(),
        "plugin lock entry should be removed"
    );
}

#[test]
fn sync_global_scope_installs_legacy_local_delegated_plugin_and_normalizes_lock() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    write_plugin_manifest(repo.path(), "pptx", "pptx@awesome-copilot");

    let output = cpm_bin()
        .args(["sync", "--scope", "global"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm sync");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = std::fs::read_to_string(&log_path).expect("read log");
    assert!(
        log.contains("plugin install pptx@awesome-copilot"),
        "sync --scope global should still install legacy delegated plugins; log: {log}"
    );
    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert_eq!(lock.plugins[0].scope, Scope::Global);
    assert_eq!(lock.plugins[0].source.scope, Scope::Global);
}

#[test]
fn demote_rejects_delegated_plugins() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    let add_output = cpm_bin()
        .args(["add", "pptx@awesome-copilot", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("seed add");
    assert!(add_output.status.success(), "seed add failed");

    let output = cpm_bin()
        .args(["demote", "pptx", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm demote");

    assert!(!output.status.success(), "demote should fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("can only use global scope"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sync_plugin_tree_source_installs_natively_without_delegate() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let plugin_dir = repo.path().join("plugins/testing-automation");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir plugin");
    std::fs::write(plugin_dir.join("README.md"), "# Testing Automation\n").expect("write readme");
    std::fs::write(
        plugin_dir.join(".github/plugin/plugin.json"),
        r#"{"name":"testing-automation","version":"1.0.0"}"#,
    )
    .expect("write plugin json");
    write_plugin_path_manifest(
        repo.path(),
        "testing-automation",
        "plugins/testing-automation",
    );

    let output = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm sync");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.trim().is_empty(),
        "native plugin sync should not delegate: {log}"
    );
    assert!(
        repo.path()
            .join(".github/plugins/testing-automation/README.md")
            .exists(),
        "native plugin files should be materialized during sync"
    );

    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert_eq!(lock.plugins.len(), 1);
    assert!(
        !lock.plugins[0].files.is_empty(),
        "native plugin should track files"
    );
}

// ── cpm doctor ────────────────────────────────────────────────────────────────

#[test]
fn doctor_passes_on_fresh_install() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let skill_dir = dir.path().join("skills/healthy");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Healthy\n").expect("write");

    cpm_bin()
        .args(["add", skill_dir.to_str().expect("utf8"), "--skill"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(dir.path())
        .output()
        .expect("add");

    let output = cpm_bin()
        .args(["doctor"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(dir.path())
        .output()
        .expect("doctor");

    assert!(
        output.status.success(),
        "doctor should pass on fresh install"
    );
}

#[test]
fn sync_records_global_claim_for_repo() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let skill_dir = repo.path().join("skills/shared");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Shared\n").expect("write skill");
    write_global_skill_manifest(repo.path(), &skill_dir);

    let output = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("sync");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let global_lock = load_global_lockfile_from(&home.path().join(".copilot/cpm.lock"))
        .expect("load global lock");
    assert_eq!(global_lock.claims.len(), 1);
    assert_eq!(global_lock.claims[0].asset.name, "shared");
    assert_eq!(global_lock.claims[0].asset.scope, Scope::Global);
    assert_eq!(
        global_lock.claims[0].claimed_by,
        Utf8PathBuf::from_path_buf(repo.path().canonicalize().expect("canonical repo"))
            .expect("utf8 repo path")
    );
}

#[test]
fn sync_fails_when_global_asset_conflicts_with_other_repo_claim() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let skill_dir = repo.path().join("skills/shared");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Shared\n").expect("write skill");
    write_global_skill_manifest(repo.path(), &skill_dir);

    let mut global_lock = GlobalLockfile::new();
    global_lock.claims.push(make_global_claim(
        Path::new("/tmp/other-repo"),
        "sha256:conflict",
    ));
    write_global_lockfile_to(&home.path().join(".copilot/cpm.lock"), &global_lock)
        .expect("seed global lock");

    let output = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("sync");

    assert!(
        !output.status.success(),
        "sync should fail on cross-repo global conflict"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("global install conflict"),
        "expected conflict error in stderr, got: {stderr}"
    );
}

#[test]
fn sync_plugin_delegates_install_and_writes_lock() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    write_plugin_manifest(repo.path(), "pptx", "pptx@awesome-copilot");

    let output = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm sync");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Plugins: 1 installed · 0 removed · 0 updated · 0 failed"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("operation=install"));
    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert_eq!(lock.plugins.len(), 1);
    assert_eq!(lock.plugins[0].resolved_rev, "rev-install");
}

// ── cpm list ──────────────────────────────────────────────────────────────────

#[test]
fn list_includes_installed_assets() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let skill_dir = dir.path().join("skills/listed");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Listed\n").expect("write");

    cpm_bin()
        .args(["add", skill_dir.to_str().expect("utf8"), "--skill"])
        .current_dir(dir.path())
        .output()
        .expect("add");

    let output = cpm_bin()
        .args(["list"])
        .current_dir(dir.path())
        .output()
        .expect("list");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("listed"),
        "installed asset should appear in list output"
    );
    assert!(
        stdout.contains("installed:") && stdout.contains(".github/skills/listed/SKILL.md"),
        "list output should surface the install target\n{stdout}"
    );
}

#[test]
fn update_plugin_delegates_and_refreshes_lock_metadata() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    let seed_output = cpm_bin()
        .args(["add", "pptx@awesome-copilot", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("seed add");
    assert!(seed_output.status.success(), "seed add failed");
    std::fs::write(&log_path, "").expect("reset log");

    let output = cpm_bin()
        .args(["update", "pptx"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm update");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = std::fs::read_to_string(&log_path).expect("read log");
    assert!(log.contains("plugin update pptx@awesome-copilot"));
    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert_eq!(lock.plugins[0].resolved_rev, "rev-update");
    let meta = lock.plugins[0].plugin_meta.as_ref().expect("plugin meta");
    assert_eq!(meta.plugin_version.as_deref(), Some("2.0.0"));
    assert!(meta
        .plugin_json_hash
        .as_deref()
        .unwrap_or_default()
        .starts_with("sha256:"));
}

#[test]
fn overview_and_status_use_plugin_markers_when_index_is_missing() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    let add_output = cpm_bin()
        .args(["add", "pptx@awesome-copilot", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm add plugin");
    assert!(
        add_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_output.stderr)
    );

    std::fs::remove_file(home.path().join(".copilot/plugin-index.json")).expect("remove index");

    let overview_output = cpm_bin()
        .args(["overview", "--plugin", "--external", "--json"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("overview");
    assert!(
        overview_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&overview_output.stderr)
    );
    let overview_json: Value =
        serde_json::from_slice(&overview_output.stdout).expect("parse overview json");
    assert_eq!(overview_json["unmanaged_count"], 0);
    assert_eq!(overview_json["status"]["drift"], 0);
    assert_eq!(
        overview_json["locked_assets"][0]["install_target"],
        normalized_path_string(
            &home
                .path()
                .join(".copilot/installed-plugins/awesome-copilot/pptx")
        )
    );
    assert!(overview_json["external"]["unclaimed_global"]
        .as_array()
        .expect("unclaimed_global array")
        .is_empty());

    let status_output = cpm_bin()
        .args(["status", "--json"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("status");
    assert!(
        status_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status_output.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status_output.stdout).expect("parse status");
    assert_eq!(status_json, Value::Array(Vec::new()));
}

#[test]
fn reporting_surfaces_show_nested_sub_assets() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    write_reporting_fixture(&dir);

    let list_output = cpm_bin()
        .args(["list", "--plugin"])
        .current_dir(dir.path())
        .output()
        .expect("list");
    assert!(list_output.status.success());
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(
        list_stdout.contains("sub-assets: 2"),
        "parent row should surface nested asset count\n{list_stdout}"
    );
    assert!(
        list_stdout.contains("installed:") && list_stdout.contains(".copilot/plugins/partners"),
        "list output should surface the install root\n{list_stdout}"
    );
    assert!(
        list_stdout.contains("source-url: https://example.com/partners"),
        "list output should surface source URLs\n{list_stdout}"
    );
    assert!(
        list_stdout.contains("source-path: plugins/partners"),
        "list output should surface source paths\n{list_stdout}"
    );
    assert!(
        list_stdout.contains("↳ agent terraform [parent] path=partners/agents/terraform.md"),
        "list output should include nested agent details\n{list_stdout}"
    );

    let tree_output = cpm_bin()
        .args(["tree"])
        .current_dir(dir.path())
        .output()
        .expect("tree");
    assert!(tree_output.status.success());
    let tree_stdout = String::from_utf8_lossy(&tree_output.stdout);
    assert!(
        tree_stdout.contains("sub-assets:"),
        "tree output should show nested assets section\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains("installed:") && tree_stdout.contains(".copilot/plugins/partners"),
        "tree output should show install target\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains("├── agent terraform [parent] path=partners/agents/terraform.md"),
        "tree output should show nested agent path\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains("└── skill prompt-lib [parent] path=partners/skills/prompt-lib"),
        "tree output should show nested skill path\n{tree_stdout}"
    );

    let show_output = cpm_bin()
        .args(["show", "partners"])
        .current_dir(dir.path())
        .output()
        .expect("show");
    assert!(show_output.status.success());
    let show_stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(
        show_stdout.contains("sub-assets: 2"),
        "show output should report nested asset count\n{show_stdout}"
    );
    assert!(
        show_stdout.contains("agent terraform [parent] path=partners/agents/terraform.md"),
        "show output should include nested agent details\n{show_stdout}"
    );
    assert!(
        show_stdout.contains("skill prompt-lib [parent] path=partners/skills/prompt-lib"),
        "show output should include nested skill details\n{show_stdout}"
    );

    let overview_output = cpm_bin()
        .args(["overview"])
        .current_dir(dir.path())
        .output()
        .expect("overview");
    assert!(overview_output.status.success());
    let overview_stdout = String::from_utf8_lossy(&overview_output.stdout);
    assert!(
        overview_stdout.contains("Nested:    2 sub-asset(s)"),
        "overview should surface nested asset count\n{overview_stdout}"
    );
    assert!(
        overview_stdout.contains("Nested assets:"),
        "overview should list nested asset details\n{overview_stdout}"
    );
    assert!(
        overview_stdout.contains("installed:")
            && overview_stdout.contains(".copilot/plugins/partners"),
        "overview should surface install target\n{overview_stdout}"
    );
    assert!(
        overview_stdout.contains(
            "plugin [local] partners -> agent terraform [parent] path=partners/agents/terraform.md"
        ),
        "overview should show nested agent details\n{overview_stdout}"
    );
}

#[test]
fn reporting_commands_emit_json_with_install_targets() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    write_reporting_fixture(&dir);

    let list_output = cpm_bin()
        .args(["list", "--plugin", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("list json");
    assert!(list_output.status.success());
    let list_json: Value = serde_json::from_slice(&list_output.stdout).expect("parse list json");
    assert_eq!(list_json[0]["name"], "partners");
    assert!(list_json[0]["group"].is_null());
    assert_eq!(
        list_json[0]["rev"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert!(
        list_json[0]["install_target"]
            .as_str()
            .unwrap_or_default()
            .contains(".copilot/plugins/partners"),
        "list json should include install target\n{list_json}"
    );
    assert_eq!(list_json[0]["source_url"], "https://example.com/partners");
    assert_eq!(list_json[0]["source_path"], "plugins/partners");

    let tree_output = cpm_bin()
        .args(["tree", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("tree json");
    assert!(tree_output.status.success());
    let tree_json: Value = serde_json::from_slice(&tree_output.stdout).expect("parse tree json");
    assert_eq!(tree_json[0]["kind"], "Plugins");
    assert!(tree_json[0]["assets"][0]["group"].is_null());
    assert!(
        tree_json[0]["assets"][0]["install_target"]
            .as_str()
            .unwrap_or_default()
            .contains(".copilot/plugins/partners"),
        "tree json should include install target\n{tree_json}"
    );

    let overview_output = cpm_bin()
        .args(["overview", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("overview json");
    assert!(overview_output.status.success());
    let overview_json: Value =
        serde_json::from_slice(&overview_output.stdout).expect("parse overview json");
    assert!(overview_json["locked_assets"][0]["group"].is_null());
    assert!(
        overview_json["locked_assets"][0]["install_target"]
            .as_str()
            .unwrap_or_default()
            .contains(".copilot/plugins/partners"),
        "overview json should include install target\n{overview_json}"
    );
    assert_eq!(
        overview_json["locked_assets"][0]["source_url"],
        "https://example.com/partners"
    );
    assert_eq!(
        overview_json["locked_assets"][0]["source_path"],
        "plugins/partners"
    );

    let show_output = cpm_bin()
        .args(["show", "partners", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("show json");
    assert!(show_output.status.success());
    let show_json: Value = serde_json::from_slice(&show_output.stdout).expect("parse show json");
    assert!(show_json[0]["group"].is_null());
    assert_eq!(
        show_json[0]["rev"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(show_json[0]["source_url"], "https://example.com/partners");
    assert_eq!(show_json[0]["source_path"], "plugins/partners");
    assert_eq!(show_json[0]["hash"], "sha256:partners");
    assert_eq!(show_json[0]["sub_assets"][0]["ownership"], "parent");
}

#[test]
fn status_json_reports_unlocked_assets() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("skills/unlocked")).expect("mkdir");
    std::fs::write(dir.path().join("skills/unlocked/SKILL.md"), "# Unlocked\n").expect("write");

    let mut manifest = Manifest::default();
    manifest.skills.insert(
        "unlocked".to_owned(),
        AssetSource {
            url: None,
            rev: None,
            path: Some(Utf8PathBuf::from("skills/unlocked")),
            group: "default".to_owned(),
            scope: Scope::Local,
            transport: None,
            env: vec![],
            args: vec![],
            engine: None,
        },
    );
    write_manifest(&dir.path().join("cpm.toml"), &manifest).expect("write manifest");
    write_lockfile(&dir.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");

    let output = cpm_bin()
        .args(["status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("status json");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse status json");
    assert_eq!(json[0]["status"], "unlocked");
    assert_eq!(json[0]["name"], "unlocked");
}

#[test]
fn status_and_tree_include_instruction_assets() {
    let dir = tempfile::TempDir::new().expect("tempdir");

    let mut manifest = Manifest::default();
    manifest.instructions.insert(
        "shell".to_owned(),
        make_instruction_source("instructions/shell.instructions.md"),
    );
    write_manifest(&dir.path().join("cpm.toml"), &manifest).expect("write manifest");

    let mut lockfile = Lockfile::new();
    lockfile
        .instructions
        .push(make_instruction_resolved("shell"));
    write_lockfile(&dir.path().join("cpm.lock"), &lockfile).expect("write lock");

    let status_output = cpm_bin()
        .args(["status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("status json");
    assert!(status_output.status.success());
    let status_json: Value =
        serde_json::from_slice(&status_output.stdout).expect("parse status json");
    assert_eq!(status_json[0]["status"], "drift");
    assert_eq!(status_json[0]["name"], "shell");
    assert!(
        status_json[0]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("file missing"),
        "status json should report instruction drift\n{status_json}"
    );

    let tree_output = cpm_bin()
        .args(["tree"])
        .current_dir(dir.path())
        .output()
        .expect("tree");
    assert!(tree_output.status.success());
    let tree_stdout = String::from_utf8_lossy(&tree_output.stdout);
    assert!(
        tree_stdout.contains("Instructions"),
        "tree output should include an instructions section\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains("instruction [local] shell"),
        "tree output should include the instruction asset\n{tree_stdout}"
    );
    assert!(
        tree_stdout.contains(".github/instructions/shell.instructions.md"),
        "tree output should include the instruction install target\n{tree_stdout}"
    );

    let tree_json_output = cpm_bin()
        .args(["tree", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("tree json");
    assert!(tree_json_output.status.success());
    let tree_json: Value =
        serde_json::from_slice(&tree_json_output.stdout).expect("parse tree json");
    assert_eq!(tree_json[0]["kind"], "Instructions");
    assert_eq!(tree_json[0]["assets"][0]["name"], "shell");
    assert!(
        tree_json[0]["assets"][0]["install_target"]
            .as_str()
            .unwrap_or_default()
            .ends_with(".github/instructions/shell.instructions.md"),
        "tree json should include the instruction install target\n{tree_json}"
    );
}

#[test]
fn list_and_tree_json_empty_results_emit_arrays() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    write_manifest(&dir.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&dir.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");

    let list_output = cpm_bin()
        .args(["list", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("list json");
    assert!(list_output.status.success());
    let list_json: Value = serde_json::from_slice(&list_output.stdout).expect("parse list json");
    assert_eq!(list_json, Value::Array(vec![]));

    let tree_output = cpm_bin()
        .args(["tree", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("tree json");
    assert!(tree_output.status.success());
    let tree_json: Value = serde_json::from_slice(&tree_output.stdout).expect("parse tree json");
    assert_eq!(tree_json, Value::Array(vec![]));
}

#[test]
fn reset_drops_global_claim_via_canonicalized_repo_path() {
    // Regression: reset used to compare claim.claimed_by against the raw cwd
    // string, so a claim stored with the real path would not be removed when
    // reset ran from a symlink to the repo.  canonical_repo_root must be used
    // on both sides.
    #[cfg(not(unix))]
    return;

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let home = tempfile::TempDir::new().expect("home tempdir");
        let real_repo = tempfile::TempDir::new().expect("real repo tempdir");
        let link_parent = tempfile::TempDir::new().expect("link parent tempdir");
        let link_path = link_parent.path().join("link-repo");
        symlink(real_repo.path(), &link_path).expect("create symlink");

        // Canonical path of the real repo directory.
        let canonical_repo = real_repo.path().canonicalize().expect("canonicalize");

        // Write a minimal manifest with a global skill.
        let manifest_toml =
            format!("[skills]\nshared = {{ path = \"skills/shared\", scope = \"global\" }}\n");
        std::fs::write(real_repo.path().join("cpm.toml"), manifest_toml).expect("write manifest");

        // Write a lockfile with the global skill entry so reset has something to remove.
        let lock_skill = ResolvedAsset {
            name: "shared".to_owned(),
            kind: AssetKind::Skill,
            source: AssetSource {
                url: None,
                rev: None,
                path: Some(Utf8PathBuf::from("skills/shared")),
                group: "default".to_owned(),
                scope: Scope::Global,
                transport: None,
                env: vec![],
                args: vec![],
                engine: None,
            },
            resolved_rev: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            resolved_date: chrono::Utc::now(),
            hash: "sha256:shared".to_owned(),
            scope: Scope::Global,
            ownership: cpm_types::AssetOwnership::Upstream,
            files: vec![Utf8PathBuf::from("shared/SKILL.md").into()],
            executable: vec![],
            file_hashes: Default::default(),
            git: None,
            sub_assets: vec![],
            license: None,
            bin_path: None,
            compiled_path: None,
            plugin_meta: None,
        };
        let mut lockfile = Lockfile::new();
        lockfile.skills.push(lock_skill.clone());
        write_lockfile(&real_repo.path().join("cpm.lock"), &lockfile).expect("write lockfile");

        // Seed the global lock with the claim stored under the canonical real path.
        let canonical_utf8 = Utf8PathBuf::from_path_buf(canonical_repo).expect("utf8");
        let mut global_lock = GlobalLockfile::new();
        global_lock
            .claims
            .push(GlobalClaim::new(canonical_utf8, lock_skill));
        write_global_lockfile_to(&home.path().join(".copilot/cpm.lock"), &global_lock)
            .expect("write global lock");

        // Run reset from the *symlink* path — the old code would compare the
        // symlink string against the canonical path and fail to remove the claim.
        let output = cpm_bin()
            .args(["reset", "--skill", "--scope", "global", "--force"])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .current_dir(&link_path)
            .output()
            .expect("run cpm reset via symlink");

        assert!(
            output.status.success(),
            "reset via symlink should succeed\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let updated_global = load_global_lockfile_from(&home.path().join(".copilot/cpm.lock"))
            .expect("load updated global lock");
        assert!(
            updated_global.claims.is_empty(),
            "reset via symlink should have removed the global claim; claims: {:?}",
            updated_global.claims
        );
    }
}

#[test]
fn reset_dry_run_reports_unmanaged_scan_requirement_without_hard() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    write_manifest(&dir.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&dir.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");

    let output = cpm_bin()
        .args(["reset", "--dry-run"])
        .current_dir(dir.path())
        .output()
        .expect("reset dry-run");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("unmanaged installs: skipped (pass --hard to scan)"),
        "reset dry-run should explain unmanaged scan behavior\n{stdout}"
    );
}

#[test]
fn reset_hard_skips_assets_claimed_by_other_repo() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let global_skill_dir = home.path().join(".copilot/skills/shared");
    std::fs::create_dir_all(&global_skill_dir).expect("mkdir");
    std::fs::write(global_skill_dir.join("SKILL.md"), "# Shared\n").expect("write");

    write_manifest(&repo.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&repo.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");

    let mut global_lock = GlobalLockfile::new();
    global_lock.claims.push(make_global_claim(
        Path::new("/tmp/other-repo"),
        "sha256:shared",
    ));
    write_global_lockfile_to(&home.path().join(".copilot/cpm.lock"), &global_lock)
        .expect("write global lock");

    let output = cpm_bin()
        .args(["reset", "--scope", "global", "--dry-run", "--hard"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("reset dry-run hard");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("shared/"),
        "claimed global asset should not be reported as unmanaged\n{stdout}"
    );
}

#[test]
fn reset_managed_plugin_delegates_to_copilot_uninstall() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    // Seed a plugin via add so the manifest, lockfile, and plugin index are populated.
    let add_output = cpm_bin()
        .args(["add", "pptx@awesome-copilot", "--plugin"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("seed add");
    assert!(add_output.status.success(), "seed add failed");
    std::fs::write(&log_path, "").expect("reset log");

    let reset_output = cpm_bin()
        .args(["reset", "--plugin", "--force"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm reset");

    assert!(
        reset_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&reset_output.stderr)
    );
    let log = std::fs::read_to_string(&log_path).expect("read log");
    assert!(
        log.contains("plugin uninstall pptx@awesome-copilot"),
        "reset should delegate plugin removal to copilot plugin uninstall; log: {log}"
    );
    // Plugin must be removed from the local lockfile.
    let lock = load_lockfile(&repo.path().join("cpm.lock")).expect("load lock");
    assert!(
        lock.plugins.is_empty(),
        "plugin should be removed from lockfile after reset"
    );
}

#[test]
fn reset_hard_unmanaged_plugin_delegates_to_copilot_uninstall() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    // Create an unmanaged plugin directory in Copilot's modern installed-plugin root.
    let plugin_dir = home
        .path()
        .join(".copilot/installed-plugins/awesome-copilot/orphan-plugin");
    std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir");
    std::fs::write(
        plugin_dir.join(".github/plugin/plugin.json"),
        br#"{"name":"orphan-plugin"}"#,
    )
    .expect("write plugin file");

    // The repo has no plugin in its manifest/lockfile — the directory is unmanaged.
    write_manifest(&repo.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&repo.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");
    write_global_lockfile_to(
        &home.path().join(".copilot/cpm.lock"),
        &GlobalLockfile::new(),
    )
    .expect("write empty global lock");

    let reset_output = cpm_bin()
        .args([
            "reset", "--plugin", "--scope", "global", "--hard", "--force",
        ])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm reset hard");

    assert!(
        reset_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&reset_output.stderr)
    );
    let log = std::fs::read_to_string(&log_path).expect("read log");
    assert!(
        log.contains("plugin uninstall orphan-plugin@awesome-copilot"),
        "reset --hard should delegate unmanaged plugin removal to copilot plugin uninstall; log: {log}"
    );
}

/// Regression: when both a plugin directory (`<name>/`) and a marker file
/// (`<name>.installed`) exist for the same unmanaged plugin, hard reset must
/// not call `copilot plugin uninstall` twice — the second call would fail and
/// abort before writing the lockfile.
#[test]
fn reset_hard_unmanaged_plugin_dedup_prevents_double_uninstall() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let (copilot_bin, log_path) = fake_copilot_env(&home, &fake_copilot);

    // Simulate the state produced by `copilot plugin install`: both the
    // plugin directory and the *.installed marker file are present.
    let installed_plugins_dir = home
        .path()
        .join(".copilot/installed-plugins/awesome-copilot");
    let legacy_plugins_dir = home.path().join(".copilot/plugins");
    let plugin_dir = installed_plugins_dir.join("orphan-plugin");
    std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir plugin dir");
    std::fs::write(
        plugin_dir.join(".github/plugin/plugin.json"),
        br#"{"name":"orphan-plugin"}"#,
    )
    .expect("write plugin file");
    std::fs::create_dir_all(&legacy_plugins_dir).expect("mkdir legacy plugin dir");
    std::fs::write(legacy_plugins_dir.join("orphan-plugin.installed"), "").expect("write marker");

    // No lockfile entry — this plugin is fully unmanaged.
    write_manifest(&repo.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&repo.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");
    write_global_lockfile_to(
        &home.path().join(".copilot/cpm.lock"),
        &GlobalLockfile::new(),
    )
    .expect("write empty global lock");

    let reset_output = cpm_bin()
        .args([
            "reset", "--plugin", "--scope", "global", "--hard", "--force",
        ])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm reset hard");

    assert!(
        reset_output.status.success(),
        "hard reset should succeed even when both dir and marker file exist for same plugin\nstderr: {}",
        String::from_utf8_lossy(&reset_output.stderr)
    );

    let log = std::fs::read_to_string(&log_path).expect("read log");
    let uninstall_count = log
        .lines()
        .filter(|l| l.contains("plugin uninstall orphan-plugin@awesome-copilot"))
        .count();
    assert_eq!(
        uninstall_count, 1,
        "`copilot plugin uninstall orphan-plugin@awesome-copilot` must be called exactly once, got {uninstall_count}; log:\n{log}"
    );
}

#[test]
fn reset_hard_removes_stale_plugin_dirs_when_copilot_reports_not_installed() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let copilot_bin = write_not_installed_copilot(&fake_copilot);
    let log_path = home.path().join("copilot.log");

    let plugin_dir = home
        .path()
        .join(".copilot/installed-plugins/awesome-copilot/orphan-plugin");
    std::fs::create_dir_all(plugin_dir.join(".github/plugin")).expect("mkdir plugin dir");
    std::fs::write(
        plugin_dir.join(".github/plugin/plugin.json"),
        br#"{"name":"orphan-plugin"}"#,
    )
    .expect("write plugin file");

    write_manifest(&repo.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&repo.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");
    write_global_lockfile_to(
        &home.path().join(".copilot/cpm.lock"),
        &GlobalLockfile::new(),
    )
    .expect("write empty global lock");

    let reset_output = cpm_bin()
        .args([
            "reset", "--plugin", "--scope", "global", "--hard", "--force",
        ])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm reset hard");

    assert!(
        reset_output.status.success(),
        "hard reset should succeed and delete stale plugin dirs even when Copilot reports not installed\nstderr: {}",
        String::from_utf8_lossy(&reset_output.stderr)
    );
    assert!(
        !plugin_dir.exists(),
        "stale plugin dir should be deleted by hard reset"
    );
}

#[test]
fn reset_hard_removes_stale_plugin_config_entries_when_copilot_reports_not_installed() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let fake_copilot = tempfile::TempDir::new().expect("copilot tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let copilot_bin = write_not_installed_copilot(&fake_copilot);
    let log_path = home.path().join("copilot.log");
    let copilot_dir = home.path().join(".copilot");
    std::fs::create_dir_all(&copilot_dir).expect("mkdir copilot dir");
    std::fs::write(
        copilot_dir.join("config.json"),
        r#"{
  "installed_plugins": [
    {
      "name": "orphan-plugin",
      "marketplace": "awesome-copilot",
      "cache_path": "/tmp/missing-orphan-plugin"
    }
  ]
}"#,
    )
    .expect("write config");

    write_manifest(&repo.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&repo.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");
    write_global_lockfile_to(
        &home.path().join(".copilot/cpm.lock"),
        &GlobalLockfile::new(),
    )
    .expect("write empty global lock");

    let reset_output = cpm_bin()
        .args([
            "reset", "--plugin", "--scope", "global", "--hard", "--force",
        ])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CPM_COPILOT_BIN", &copilot_bin)
        .env("CPM_TEST_LOG", &log_path)
        .current_dir(repo.path())
        .output()
        .expect("run cpm reset hard");

    assert!(
        reset_output.status.success(),
        "hard reset should succeed and delete stale plugin config entries even when Copilot reports not installed\nstderr: {}",
        String::from_utf8_lossy(&reset_output.stderr)
    );

    let config_json: Value =
        serde_json::from_slice(&std::fs::read(copilot_dir.join("config.json")).expect("config"))
            .expect("parse config");
    assert!(
        config_json["installed_plugins"]
            .as_array()
            .expect("installed_plugins array")
            .is_empty(),
        "stale plugin config entry should be removed"
    );
}

#[test]
fn reset_global_asset_drops_claim_from_global_lockfile() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    let skill_dir = repo.path().join("skills/shared");
    std::fs::create_dir_all(&skill_dir).expect("mkdir");
    std::fs::write(skill_dir.join("SKILL.md"), "# Shared\n").expect("write skill");
    write_global_skill_manifest(repo.path(), &skill_dir);

    // Seed a global skill via sync so the global lockfile has a claim.
    let sync_output = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("seed sync");
    assert!(
        sync_output.status.success(),
        "seed sync failed: {}",
        String::from_utf8_lossy(&sync_output.stderr)
    );

    let global_lock_path = home.path().join(".copilot/cpm.lock");
    let before = load_global_lockfile_from(&global_lock_path).expect("load before");
    assert_eq!(
        before.claims.len(),
        1,
        "global lock should have one claim after sync"
    );

    let reset_output = cpm_bin()
        .args(["reset", "--skill", "--scope", "global", "--force"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("run cpm reset");

    assert!(
        reset_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&reset_output.stderr)
    );

    let after = load_global_lockfile_from(&global_lock_path).expect("load after");
    assert!(
        after.claims.is_empty(),
        "reset should drop the global claim from the global lockfile; claims: {:?}",
        after.claims
    );
}

#[test]
fn overview_reports_unmanaged_global_mcp_entries_from_config() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let repo = tempfile::TempDir::new().expect("repo tempdir");
    std::fs::create_dir_all(home.path().join(".copilot")).expect("mkdir");
    std::fs::write(
        home.path().join(".copilot/mcp-config.json"),
        r#"{ "mcpServers": { "external-server": { "type": "http", "url": "https://example.com/mcp" } } }"#,
    )
    .expect("write mcp config");

    write_manifest(&repo.path().join("cpm.toml"), &Manifest::default()).expect("write manifest");
    write_lockfile(&repo.path().join("cpm.lock"), &Lockfile::new()).expect("write lock");

    let output = cpm_bin()
        .args(["overview", "--mcp", "--scope", "global", "--json"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(repo.path())
        .output()
        .expect("overview json");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse overview json");
    assert_eq!(json["unmanaged_count"], 1);
    assert_eq!(json["unmanaged"][0]["entry_type"], "mcp-server");
    assert_eq!(
        json["unmanaged"][0]["path"],
        home.path()
            .join(".copilot/mcp-config.json")
            .display()
            .to_string()
            + "#external-server"
    );
}

// ── cpm cache ────────────────────────────────────────────────────────────────

#[test]
fn cache_dir_prints_non_empty_path() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let output = cpm_bin()
        .args(["cache", "dir"])
        .current_dir(dir.path())
        .output()
        .expect("cache dir");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    assert!(!stdout.is_empty(), "cache dir should print a path");
}

// ── cpm auth ─────────────────────────────────────────────────────────────────

#[test]
fn auth_status_exits_zero() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let output = cpm_bin()
        .args(["auth", "status"])
        .current_dir(dir.path())
        .output()
        .expect("auth status");
    assert!(output.status.success());
}

#[test]
fn auth_login_without_token_prints_clear_guidance() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let output = cpm_bin()
        .args(["auth", "login"])
        .env_remove("CPM_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .current_dir(dir.path())
        .output()
        .expect("auth login");

    assert!(
        !output.status.success(),
        "login should fail without a token"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No GitHub token found"));
    assert!(stderr.contains("https://github.com/settings/personal-access-tokens/new"));
    assert!(stderr.contains("CPM_TOKEN=ghp_your_token uv run cpm auth login"));
    assert!(stderr.contains("--open"));
}

// ── sync stale-asset cleanup ──────────────────────────────────────────────────

/// After removing a skill from cpm.toml, `cpm sync` must delete the previously
/// installed files so the disk state converges back to what the manifest says.
#[test]
fn sync_removes_stale_local_skill_when_removed_from_manifest() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let dir = tempfile::TempDir::new().expect("repo dir");

    // Create a source skill directory with a real file.
    let skill_src = dir.path().join("src-skills/vanishing");
    std::fs::create_dir_all(&skill_src).expect("mkdir skill src");
    std::fs::write(skill_src.join("SKILL.md"), "# Vanishing\n").expect("write skill");

    // Add the skill — this installs it to .github/skills/vanishing/SKILL.md.
    let add_out = cpm_bin()
        .args(["add", skill_src.to_str().expect("utf8"), "--skill"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(dir.path())
        .output()
        .expect("cpm add");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let installed_file = dir.path().join(".github/skills/vanishing/SKILL.md");
    assert!(
        installed_file.exists(),
        "skill must be installed before the test"
    );

    // Remove the skill from the manifest while leaving the lock intact so that
    // sync can detect the staleness.
    std::fs::write(dir.path().join("cpm.toml"), "[skills]\n").expect("clear manifest");

    // sync should detect that `vanishing` is gone from the manifest and remove
    // the installed file.
    let sync_out = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(dir.path())
        .output()
        .expect("cpm sync");
    assert!(
        sync_out.status.success(),
        "sync failed: {}",
        String::from_utf8_lossy(&sync_out.stderr)
    );

    assert!(
        !installed_file.exists(),
        "stale skill file should have been removed by sync"
    );
}

/// When a skill's scope moves from local to global, `cpm sync` should remove
/// the old local install and materialise the asset in the global location.
#[test]
fn sync_removes_stale_local_install_when_skill_scope_changes_to_global() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let dir = tempfile::TempDir::new().expect("repo dir");

    // Create a source skill directory.
    let skill_src = dir.path().join("src-skills/shared");
    std::fs::create_dir_all(&skill_src).expect("mkdir skill src");
    std::fs::write(skill_src.join("SKILL.md"), "# Shared\n").expect("write skill");

    // First install: local scope (default for `cpm add <path>`).
    let add_out = cpm_bin()
        .args(["add", skill_src.to_str().expect("utf8"), "--skill"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(dir.path())
        .output()
        .expect("cpm add local");
    assert!(
        add_out.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let local_file = dir.path().join(".github/skills/shared/SKILL.md");
    assert!(
        local_file.exists(),
        "local skill must be present before scope change"
    );

    // Rewrite the manifest to use global scope.
    let manifest_str = format!(
        "[skills]\nshared = {{ path = \"{}\", scope = \"global\" }}\n",
        normalized_path_string(&skill_src)
    );
    std::fs::write(dir.path().join("cpm.toml"), &manifest_str).expect("rewrite manifest");

    // sync should clean up the local install and write the global one.
    let sync_out = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(dir.path())
        .output()
        .expect("cpm sync");
    assert!(
        sync_out.status.success(),
        "sync failed: {}",
        String::from_utf8_lossy(&sync_out.stderr)
    );

    assert!(
        !local_file.exists(),
        "old local install should have been removed after scope change to global"
    );

    let global_file = home.path().join(".copilot/skills/shared/SKILL.md");
    assert!(
        global_file.exists(),
        "new global install should exist after scope change"
    );
}

// ── sync progress reporting ───────────────────────────────────────────────────

/// `cpm sync` must emit structured progress lines on stderr for non-plugin
/// asset installs so that consumers can parse install events.
#[test]
fn sync_reports_progress_for_non_plugin_skill_install() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let dir = tempfile::TempDir::new().expect("repo dir");

    let skill_src = dir.path().join("src-skills/tracked");
    std::fs::create_dir_all(&skill_src).expect("mkdir skill src");
    std::fs::write(skill_src.join("SKILL.md"), "# Tracked\n").expect("write skill");

    // Write the manifest only — no lockfile — so sync must resolve and install.
    let manifest_str = format!(
        "[skills]\ntracked = {{ path = \"{}\" }}\n",
        normalized_path_string(&skill_src)
    );
    std::fs::write(dir.path().join("cpm.toml"), &manifest_str).expect("write manifest");

    let output = cpm_bin()
        .args(["sync"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        // Ensure CI env is unset so plain progress lines are emitted to stderr
        // regardless of whether there is a terminal.
        .env_remove("CI")
        .current_dir(dir.path())
        .output()
        .expect("cpm sync");

    assert!(
        output.status.success(),
        "sync failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cpm-progress") && stderr.contains("operation=install"),
        "expected progress lines in stderr for skill install, got:\n{stderr}"
    );
}
