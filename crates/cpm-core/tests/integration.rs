//! Integration tests for cpm-core: manifest, lockfile, installer, doctor, status.

use std::path::Path;

use cpm_core::{
    doctor::run_doctor,
    installer::{
        copilot_mcp_config_path, copilot_server_entry, install_asset, install_dir, mcp_json,
        remove_asset,
    },
    paths::join_portable_path,
    project::{load_lockfile, load_manifest, write_lockfile, write_manifest},
    resolver::{check_lock_freshness, detect_conflicts},
    status::{check_status, AssetStatus},
};
use cpm_types::{
    AssetKind, AssetOwnership, AssetSource, EnvSpec, Lockfile, Manifest, McpTransport,
    ResolvedAsset, Scope,
};
use tempfile::TempDir;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_source(url: &str) -> AssetSource {
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
    }
}

fn make_resolved(name: &str, kind: AssetKind, scope: Scope) -> ResolvedAsset {
    ResolvedAsset {
        name: name.to_owned(),
        kind,
        source: make_source("https://example.com/repo"),
        resolved_rev: "a".repeat(40),
        resolved_date: chrono::Utc::now(),
        hash: "sha256:abc123".to_owned(),
        scope,
        ownership: AssetOwnership::Upstream,
        files: vec![],
        executable: vec![],
        file_hashes: Default::default(),
        git: None,
        sub_assets: vec![],
        bin_path: None,
        compiled_path: None,
        plugin_meta: None,
        license: None,
    }
}

// ── Manifest I/O ─────────────────────────────────────────────────────────────

#[test]
fn manifest_round_trips_through_file() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("cpm.toml");

    let mut manifest = Manifest::default();
    manifest
        .plugins
        .insert("my-plugin".to_owned(), make_source("https://example.com/p"));
    manifest
        .skills
        .insert("my-skill".to_owned(), make_source("https://example.com/s"));

    write_manifest(&path, &manifest).expect("write_manifest");
    let loaded = load_manifest(&path).expect("load_manifest");

    assert!(loaded.plugins.contains_key("my-plugin"));
    assert!(loaded.skills.contains_key("my-skill"));
}

#[test]
fn checked_in_manifests_load_successfully() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonical repo root");

    let repo_manifest = load_manifest(&repo_root.join("cpm.toml")).expect("load repo manifest");
    let reference_manifest =
        load_manifest(&repo_root.join("cpm-reference.toml")).expect("load reference manifest");

    let local_paths: Vec<_> = repo_manifest
        .plugins
        .values()
        .chain(repo_manifest.skills.values())
        .chain(repo_manifest.agents.values())
        .chain(repo_manifest.hooks.values())
        .chain(repo_manifest.workflows.values())
        .filter_map(|source| source.path.as_ref())
        .collect();
    for path in local_paths {
        assert!(
            !path.is_absolute(),
            "checked-in local manifest paths should stay repo-relative",
        );
    }

    let slack_notify = reference_manifest
        .hooks
        .get("slack-notify")
        .expect("slack-notify hook");
    assert!(
        slack_notify
            .env
            .iter()
            .any(|spec| spec.key == "SLACK_WEBHOOK_URL"),
        "reference manifest should retain hook env configuration",
    );
    assert!(reference_manifest.mcps.contains_key("github"));
}

// ── Lockfile I/O ─────────────────────────────────────────────────────────────

#[test]
fn lockfile_round_trips_through_file() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("cpm.lock");

    let mut lockfile = Lockfile::new();
    lockfile
        .skills
        .push(make_resolved("alpha", AssetKind::Skill, Scope::Local));

    write_lockfile(&path, &lockfile).expect("write_lockfile");
    let loaded = load_lockfile(&path).expect("load_lockfile");

    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.skills.len(), 1);
    assert_eq!(loaded.skills[0].name, "alpha");
}

#[test]
fn empty_lockfile_has_version_1() {
    let lf = Lockfile::new();
    assert_eq!(lf.version, 1);
}

// ── Conflict detection ────────────────────────────────────────────────────────

#[test]
fn no_conflict_different_names() {
    let mut lf = Lockfile::new();
    lf.mcps
        .push(make_resolved("mcp-a", AssetKind::Mcp, Scope::Local));
    lf.mcps
        .push(make_resolved("mcp-b", AssetKind::Mcp, Scope::Global));
    assert!(detect_conflicts(&lf).is_ok());
}

#[test]
fn no_conflict_same_name_different_kind() {
    let mut lf = Lockfile::new();
    lf.plugins
        .push(make_resolved("shared", AssetKind::Plugin, Scope::Local));
    lf.skills
        .push(make_resolved("shared", AssetKind::Skill, Scope::Global));
    assert!(detect_conflicts(&lf).is_ok());
}

#[test]
fn conflict_detected_same_name_same_kind_both_scopes() {
    let mut lf = Lockfile::new();
    lf.mcps
        .push(make_resolved("conflict-mcp", AssetKind::Mcp, Scope::Local));
    lf.mcps
        .push(make_resolved("conflict-mcp", AssetKind::Mcp, Scope::Global));
    let err = detect_conflicts(&lf).expect_err("should conflict");
    assert!(matches!(err, cpm_core::CpmError::ScopeConflict { .. }));
}

// ── Lock freshness ────────────────────────────────────────────────────────────

#[test]
fn fresh_lock_when_manifest_is_empty() {
    let manifest = Manifest::default();
    let lockfile = Lockfile::new();
    assert!(check_lock_freshness(&manifest, &lockfile).is_ok());
}

#[test]
fn stale_lock_when_manifest_has_entry_not_in_lock() {
    let mut manifest = Manifest::default();
    manifest
        .plugins
        .insert("new".to_owned(), make_source("https://example.com"));
    let lockfile = Lockfile::new();
    let err = check_lock_freshness(&manifest, &lockfile).expect_err("should be stale");
    assert!(matches!(err, cpm_core::CpmError::LockOutOfDate));
}

#[test]
fn lock_fresh_when_all_manifest_entries_are_locked() {
    let mut manifest = Manifest::default();
    manifest
        .plugins
        .insert("p".to_owned(), make_source("https://example.com"));

    let mut lockfile = Lockfile::new();
    let mut resolved = make_resolved("p", AssetKind::Plugin, Scope::Local);
    // The lock source must match the manifest source exactly.
    resolved.source = make_source("https://example.com");
    lockfile.plugins.push(resolved);

    assert!(check_lock_freshness(&manifest, &lockfile).is_ok());
}

// ── Installer ────────────────────────────────────────────────────────────────

#[test]
fn install_dir_local_paths_by_kind() {
    let root = Path::new("/repo");
    assert_eq!(
        install_dir(AssetKind::Plugin, Scope::Local, root),
        join_portable_path(root, ".github/plugins")
    );
    assert_eq!(
        install_dir(AssetKind::Skill, Scope::Local, root),
        join_portable_path(root, ".github/skills")
    );
    assert_eq!(
        install_dir(AssetKind::Agent, Scope::Local, root),
        join_portable_path(root, ".github/agents")
    );
    // MCP assets are written into the aggregate Copilot config, not a bare
    // directory.  Verify the dedicated path helper instead.
    assert_eq!(
        copilot_mcp_config_path(Scope::Local, root),
        join_portable_path(root, ".vscode/mcp.json")
    );
}

#[test]
fn mcp_json_secret_env_values_omitted() {
    let env = vec![
        EnvSpec::from_raw("LITERAL_KEY", "plain_value"),
        EnvSpec::from_raw("SECRET_KEY", "$SECRET_ENV_VAR"),
    ];
    let mut asset = make_resolved("test-mcp", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Npx {
        package: "@org/pkg".to_owned(),
        entrypoint: None,
        args: vec![],
    });
    asset.source.env = env;

    let json = mcp_json(&asset).expect("json");
    assert_eq!(json["type"], "npx");
    let env_obj = json.get("env").expect("env key");
    assert!(
        env_obj.get("LITERAL_KEY").is_some(),
        "literal key should be present"
    );
    assert_eq!(
        env_obj.get("SECRET_KEY").and_then(|v| v.as_str()),
        Some("${env:SECRET_ENV_VAR}"),
        "FromEnv must be written as ${{env:VAR}} substitution"
    );
}

#[test]
fn install_and_remove_mcp_json() {
    let dir = TempDir::new().expect("tempdir");
    let mut asset = make_resolved("my-mcp", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Http {
        url: "https://api.example.com/mcp".to_owned(),
    });

    install_asset(&asset, dir.path()).expect("install");

    // Local MCP config lives in .vscode/mcp.json (Copilot workspace format).
    let config_path = join_portable_path(dir.path(), ".vscode/mcp.json");
    assert!(
        config_path.exists(),
        ".vscode/mcp.json should exist after install"
    );

    let raw = std::fs::read_to_string(&config_path).expect("read config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse config");
    assert_eq!(v["servers"]["my-mcp"]["type"], "http");
    assert_eq!(v["servers"]["my-mcp"]["url"], "https://api.example.com/mcp");

    remove_asset(&asset, dir.path()).expect("remove");

    // Config file is retained; entry is removed.
    assert!(config_path.exists(), "config file should survive removal");
    let raw = std::fs::read_to_string(&config_path).expect("read after remove");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse after remove");
    assert!(
        v["servers"].get("my-mcp").is_none(),
        "my-mcp entry should be removed"
    );
}

#[test]
fn install_mcp_upserts_multiple_servers_into_one_file() {
    let dir = TempDir::new().expect("tempdir");

    let mut asset_http = make_resolved("http-server", AssetKind::Mcp, Scope::Local);
    asset_http.source.transport = Some(McpTransport::Http {
        url: "https://http.example.com/mcp".to_owned(),
    });

    let mut asset_npx = make_resolved("npx-server", AssetKind::Mcp, Scope::Local);
    asset_npx.source.transport = Some(McpTransport::Npx {
        package: "@org/mcp-pkg".to_owned(),
        entrypoint: None,
        args: vec!["--verbose".to_owned()],
    });

    install_asset(&asset_http, dir.path()).expect("install http");
    install_asset(&asset_npx, dir.path()).expect("install npx");

    let config_path = join_portable_path(dir.path(), ".vscode/mcp.json");
    let raw = std::fs::read_to_string(&config_path).expect("read config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse config");

    assert_eq!(v["servers"]["http-server"]["type"], "http");
    assert_eq!(v["servers"]["npx-server"]["type"], "stdio");
    assert_eq!(v["servers"]["npx-server"]["command"], "npx");

    let args = v["servers"]["npx-server"]["args"].as_array().expect("args");
    assert_eq!(args[0], "-y");
    assert_eq!(args[1], "@org/mcp-pkg");
    assert_eq!(args[2], "--verbose");
}

#[test]
fn remove_mcp_leaves_other_servers_intact() {
    let dir = TempDir::new().expect("tempdir");

    let mut asset_a = make_resolved("server-a", AssetKind::Mcp, Scope::Local);
    asset_a.source.transport = Some(McpTransport::Http {
        url: "https://a.example.com/mcp".to_owned(),
    });
    let mut asset_b = make_resolved("server-b", AssetKind::Mcp, Scope::Local);
    asset_b.source.transport = Some(McpTransport::Http {
        url: "https://b.example.com/mcp".to_owned(),
    });

    install_asset(&asset_a, dir.path()).expect("install a");
    install_asset(&asset_b, dir.path()).expect("install b");
    remove_asset(&asset_a, dir.path()).expect("remove a");

    let config_path = join_portable_path(dir.path(), ".vscode/mcp.json");
    let raw = std::fs::read_to_string(&config_path).expect("read");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
    assert!(v["servers"].get("server-a").is_none(), "server-a removed");
    assert!(v["servers"].get("server-b").is_some(), "server-b intact");
}

#[test]
fn remove_mcp_is_no_op_when_config_absent() {
    let dir = TempDir::new().expect("tempdir");
    let mut asset = make_resolved("ghost-mcp", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Http {
        url: "https://example.com/mcp".to_owned(),
    });
    // Removing without ever installing should succeed silently.
    remove_asset(&asset, dir.path()).expect("no-op remove should not error");
    assert!(!join_portable_path(dir.path(), ".vscode/mcp.json").exists());
}

#[test]
fn copilot_server_entry_npx_shape() {
    let env = vec![
        EnvSpec::from_raw("LITERAL_KEY", "plain_value"),
        EnvSpec::from_raw("SECRET_KEY", "$SECRET_ENV_VAR"),
    ];
    let mut asset = make_resolved("test-npx", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Npx {
        package: "@org/pkg".to_owned(),
        entrypoint: None,
        args: vec![],
    });
    asset.source.env = env;

    let entry = copilot_server_entry(&asset).expect("entry");
    assert_eq!(entry["type"], "stdio");
    assert_eq!(entry["command"], "npx");
    // FromEnv values serialized as ${env:VAR} references.
    let env_obj = entry.get("env").expect("env");
    assert!(env_obj.get("LITERAL_KEY").is_some());
    assert_eq!(
        env_obj.get("SECRET_KEY").and_then(|v| v.as_str()),
        Some("${env:SECRET_ENV_VAR}"),
    );
}

#[test]
fn copilot_server_entry_uvx_shape() {
    let mut asset = make_resolved("test-uvx", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Uvx {
        package: "mcp-server-time".to_owned(),
        entrypoint: None,
        args: vec!["--local-timezone=UTC".to_owned()],
    });

    let entry = copilot_server_entry(&asset).expect("entry");
    assert_eq!(entry["type"], "stdio");
    assert_eq!(entry["command"], "uvx");
    let args = entry["args"].as_array().expect("args");
    assert_eq!(args[0], "mcp-server-time");
    assert_eq!(args[1], "--local-timezone=UTC");
}

#[test]
fn copilot_server_entry_docker_shape() {
    let mut asset = make_resolved("test-docker", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Docker {
        image: "ghcr.io/org/mcp:latest".to_owned(),
        args: vec![],
    });

    let entry = copilot_server_entry(&asset).expect("entry");
    assert_eq!(entry["type"], "stdio");
    assert_eq!(entry["command"], "docker");
    let args = entry["args"].as_array().expect("args");
    assert_eq!(args[0], "run");
    assert_eq!(args[1], "--rm");
    assert_eq!(args[2], "-i");
    assert_eq!(args[3], "ghcr.io/org/mcp:latest");
}

#[test]
fn copilot_server_entry_script_shape() {
    let mut asset = make_resolved("test-script", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Script {
        command: "node ./server.js".to_owned(),
        args: vec![],
    });

    let entry = copilot_server_entry(&asset).expect("entry");
    assert_eq!(entry["type"], "stdio");
    assert_eq!(entry["command"], "sh");
    let args = entry["args"].as_array().expect("args");
    assert_eq!(args[0], "-c");
    assert_eq!(args[1], "node ./server.js");
}

#[test]
fn copilot_server_entry_http_no_env_key_when_empty() {
    let mut asset = make_resolved("test-http", AssetKind::Mcp, Scope::Local);
    asset.source.transport = Some(McpTransport::Http {
        url: "https://example.com/mcp".to_owned(),
    });

    let entry = copilot_server_entry(&asset).expect("entry");
    assert_eq!(entry["type"], "http");
    assert_eq!(entry["url"], "https://example.com/mcp");
    // No `env` key for HTTP with no env vars.
    assert!(entry.get("env").is_none());
}

#[test]
fn global_mcp_config_path_ends_with_copilot_mcp_config_json() {
    // We cannot write to the real ~/.copilot in CI, so we just verify the
    // path helper returns the expected pattern.
    let fake_root = Path::new("/fake-home");
    let global_path = copilot_mcp_config_path(Scope::Global, fake_root);
    let path_str = cpm_core::paths::portable_path_string(&global_path);
    assert!(
        path_str.ends_with(".copilot/mcp-config.json"),
        "global path should end with .copilot/mcp-config.json, got {path_str}"
    );
}

// ── Doctor ────────────────────────────────────────────────────────────────────

#[test]
fn doctor_empty_lockfile_is_clean() {
    let dir = TempDir::new().expect("tempdir");
    let lf = Lockfile::new();
    let errors = run_doctor(&lf, dir.path()).expect("doctor");
    assert!(errors.is_empty(), "empty lockfile should produce no errors");
}

#[test]
fn doctor_detects_missing_file() {
    let dir = TempDir::new().expect("tempdir");
    let mut asset = make_resolved("ghost", AssetKind::Plugin, Scope::Local);
    asset.files = vec![camino::Utf8PathBuf::from("ghost.yml").into()];
    asset.hash = "sha256:anything".to_owned();

    let mut lf = Lockfile::new();
    lf.plugins.push(asset);

    let errors = run_doctor(&lf, dir.path()).expect("doctor");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].actual, "<missing>");
}

// ── Status ────────────────────────────────────────────────────────────────────

#[test]
fn status_clean_when_manifest_and_lock_agree() {
    let dir = TempDir::new().expect("tempdir");
    let mut manifest = Manifest::default();
    manifest
        .plugins
        .insert("p".to_owned(), make_source("https://example.com"));

    let mut lf = Lockfile::new();
    // The locked source must exactly match the manifest source for a clean result.
    let mut resolved = make_resolved("p", AssetKind::Plugin, Scope::Local);
    resolved.source = make_source("https://example.com");
    lf.plugins.push(resolved);

    let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
    // An empty list means everything is clean — check_status never pushes a
    // Clean variant; callers infer "clean" from an empty return value.
    assert!(
        statuses.is_empty(),
        "expected clean (empty list), got: {statuses:?}"
    );
}

#[test]
fn status_unlocked_when_entry_missing_from_lock() {
    let dir = TempDir::new().expect("tempdir");
    let mut manifest = Manifest::default();
    manifest
        .plugins
        .insert("new-plugin".to_owned(), make_source("https://example.com"));

    let lf = Lockfile::new();
    let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
    assert!(statuses
        .iter()
        .any(|s| matches!(s, AssetStatus::Unlocked { .. })));
}

#[test]
fn status_stale_when_rev_differs() {
    let dir = TempDir::new().expect("tempdir");
    let mut source = make_source("https://example.com");
    source.rev = Some("v99.0.0".to_owned());

    let mut manifest = Manifest::default();
    manifest.plugins.insert("p".to_owned(), source.clone());

    let mut lf = Lockfile::new();
    let mut resolved = make_resolved("p", AssetKind::Plugin, Scope::Local);
    // The lock source must match the manifest source (minus rev) for the stale
    // check to trigger only on the rev difference.
    resolved.source = source;
    lf.plugins.push(resolved);

    let statuses = check_status(&manifest, &lf, dir.path()).expect("status");
    assert!(statuses
        .iter()
        .any(|s| matches!(s, AssetStatus::Stale { .. })));
}
