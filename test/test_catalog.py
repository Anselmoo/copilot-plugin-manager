import re
import tomllib
from pathlib import Path

from rich.console import Console

from copilot_plugin_manager.catalog import load_catalog_bundle
from copilot_plugin_manager.rendering import _ordered_profiles, _profile_focus, build_target_tree, render_plugins, render_providers, render_themes

CATALOG_DATA_DIR = Path(__file__).resolve().parents[1] / "src" / "copilot_plugin_manager" / "catalog_data"


def test_catalog_bundle_counts() -> None:
    bundle = load_catalog_bundle()
    assert len(bundle.plugins) > 0
    assert len(bundle.repositories) == 7
    assert len(bundle.skill_providers) == 210
    assert len(bundle.agent_providers) == 58
    assert len(bundle.entrypoints) > len(bundle.plugins)
    assert len(bundle.themes) == 30
    assert len(bundle.profiles) == 39


def test_resolve_known_profile() -> None:
    bundle = load_catalog_bundle()
    target = bundle.resolve_target("python-mcp")
    assert target.kind == "profile"
    assert target.themes == ["core", "mcp", "mcp-agents", "python", "python-agents", "testing"]


def test_resolve_added_curated_profiles() -> None:
    bundle = load_catalog_bundle()

    assert bundle.resolve_target("ts").themes == ["core", "frontend", "testing", "typescript"]
    assert bundle.resolve_target("ts-mcp").themes == ["core", "frontend", "mcp", "mcp-agents", "testing", "typescript"]
    assert bundle.resolve_target("python-cloud").themes == ["core", "devops", "python", "python-cloud", "testing"]
    assert bundle.resolve_target("python-plus-rust").themes == ["core", "data", "python", "rust", "testing"]
    assert bundle.resolve_target("pydantic").themes == ["core", "openapi", "python", "testing"]
    assert bundle.resolve_target("fastapi-typer").themes == ["core", "openapi", "python", "testing"]
    assert bundle.resolve_target("backend").themes == bundle.resolve_target("backend-api").themes
    assert bundle.resolve_target("scientific-programming").themes == ["core", "data", "python", "research", "science"]


def test_rust_theme_exposes_rust_specific_content() -> None:
    bundle = load_catalog_bundle()
    rust_theme = bundle.themes["rust"]

    assert "rust-mcp-development" in rust_theme.plugins
    assert "mskills-rust" in rust_theme.skills


def test_python_themes_separate_generic_and_cloud_skill_packs() -> None:
    bundle = load_catalog_bundle()
    python_theme = bundle.themes["python"]
    python_cloud_theme = bundle.themes["python-cloud"]

    assert python_theme.skills == ["anthropic-claude-api", "mskills-python"]
    assert python_cloud_theme.skills == [
        "mskills-python-data",
        "mskills-python-foundry",
        "mskills-python-integration",
        "mskills-python-monitoring",
    ]
    assert "azure-cloud-development" in python_cloud_theme.plugins


def test_docs_themes_keep_doc_tools_and_visual_artifacts_separate() -> None:
    bundle = load_catalog_bundle()
    docs_theme = bundle.themes["docs"]
    docs_design_theme = bundle.themes["docs-design"]

    assert "mskills-typescript-frontend" not in docs_theme.skills
    assert "anthropic-slack-gif-creator" in docs_design_theme.skills


def test_plugin_and_repository_metadata_are_enriched() -> None:
    bundle = load_catalog_bundle()
    plugin = bundle.plugin_details("python-mcp-development")
    repository = bundle.repository_metadata("awesome-copilot")

    assert plugin["version"] == "marketplace-latest"
    assert "python" in plugin["tags"]
    assert plugin["source_url"].startswith("https://github.com/")
    assert repository["url"].startswith("https://github.com/")
    assert "plugins" in repository["tags"]


def test_catalog_prefers_persisted_plugin_metadata() -> None:
    bundle = load_catalog_bundle()
    plugin = bundle.plugin_details("python-mcp-development")
    raw = bundle.plugins["python-mcp-development"]

    assert raw.description is not None
    assert raw.source_url is not None
    assert raw.version_channel == "marketplace-latest"
    assert "python" in raw.tags

    assert plugin["description"] == raw.description
    assert plugin["source_url"] == raw.source_url
    assert plugin["version"] == raw.version_channel
    assert plugin["tags"] == ", ".join(raw.tags)


def test_catalog_exposes_entrypoint_metadata() -> None:
    bundle = load_catalog_bundle()
    entrypoint = bundle.entrypoint_for_path(
        "agent",
        "agency-agents",
        "design/design-brand-guardian.md",
        provider="agency-design-brand-guardian",
    )

    assert entrypoint is not None
    assert entrypoint.local_output.endswith(".agent.md")
    assert entrypoint.measured_revision is not None
    assert entrypoint.measured_at is not None
    assert entrypoint.commit_revision is not None
    assert entrypoint.commit_date is not None


def test_catalog_prefers_more_specific_agent_providers_first() -> None:
    bundle = load_catalog_bundle()

    ordered = bundle.preferred_provider_order(
        "agent",
        ["agency", "agency-design", "agency-design-brand-guardian"],
    )

    assert ordered == [
        "agency-design-brand-guardian",
        "agency-design",
        "agency",
    ]


def test_target_items_are_sorted_alphabetically_after_deduplication() -> None:
    bundle = load_catalog_bundle()

    assert bundle.target_items(["docs", "docs-design"], "skills") == [
        "anthropic-algorithmic-art",
        "anthropic-brand-guidelines",
        "anthropic-canvas-design",
        "anthropic-doc-coauthoring",
        "anthropic-docx",
        "anthropic-frontend-design",
        "anthropic-internal-comms",
        "anthropic-pdf",
        "anthropic-pptx",
        "anthropic-slack-gif-creator",
        "anthropic-theme-factory",
        "anthropic-xlsx",
    ]


def test_catalog_summaries_surface_revisions_and_breakdown() -> None:
    bundle = load_catalog_bundle()

    source_summary = bundle.source_entrypoint_summary("agency-agents")
    provider_summary = bundle.provider_entrypoint_summary("agent", "voltagent-code-reviewer")

    assert source_summary["revision"] is not None
    assert source_summary["commit_date"] is not None
    assert int(source_summary["file_count"] or 0) > 0
    assert provider_summary["layout"] == "single-file"
    assert provider_summary["revision"] is not None
    assert provider_summary["entrypoint_count"] == 1


def test_agent_provider_metadata_is_extracted_from_target_files() -> None:
    bundle = load_catalog_bundle()
    provider = bundle.provider_details("agent", "voltagent-code-reviewer")

    assert "category or specialist subagents" not in provider["description"]
    assert "specific VoltAgent specialist or workflow agent" not in provider["use_when"]
    assert "code review" in provider["description"].lower()
    assert "use when you need" in provider["use_when"].lower()


def test_themes_only_reference_catalogued_plugins() -> None:
    bundle = load_catalog_bundle()

    missing = {
        theme_name: [plugin_name for plugin_name in theme.plugins if plugin_name not in bundle.plugins]
        for theme_name, theme in bundle.themes.items()
        if any(plugin_name not in bundle.plugins for plugin_name in theme.plugins)
    }

    assert missing == {}


def test_kdense_skill_provider_roots_include_scientific_skills_prefix() -> None:
    bundle = load_catalog_bundle()

    offenders = {
        name: provider.roots
        for name, provider in bundle.skill_providers.items()
        if provider.source == "kdense-science" and any(not root.startswith("scientific-skills/") for root in provider.roots)
    }

    assert offenders == {}


def test_catalog_skill_entrypoints_track_real_skill_roots() -> None:
    bundle = load_catalog_bundle()

    anthropic_entry = bundle.entrypoint_for_path(
        "skill",
        "anthropics-skills",
        "skills/claude-api",
        provider="anthropic-claude-api",
    )
    kdense_entry = bundle.entrypoint_for_path(
        "skill",
        "kdense-science",
        "scientific-skills/arboreto",
        provider="kdense-arboreto",
    )

    assert anthropic_entry is not None
    assert anthropic_entry.source_url.endswith("/skills/claude-api")
    assert bundle.entrypoint_for_path("skill", "anthropics-skills", "skills/claude-api/python", provider="anthropic-claude-api") is None

    assert kdense_entry is not None
    assert kdense_entry.source_url.endswith("/scientific-skills/arboreto")
    assert bundle.entrypoint_for_path("skill", "kdense-science", "scientific-skills/arboreto/scripts", provider="kdense-arboreto") is None


def test_catalog_mcp_monorepo_links_point_to_server_subtrees() -> None:
    bundle = load_catalog_bundle()

    assert bundle.mcp_details("filesystem")["source_url"] == "https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem"
    assert bundle.mcp_details("memory")["source_url"] == "https://github.com/modelcontextprotocol/servers/tree/main/src/memory"
    assert bundle.mcp_details("sequential-thinking")["source_url"] == "https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking"
    assert bundle.mcp_details("postgres")["source_url"] == "https://github.com/modelcontextprotocol/servers-archived/tree/main/src/postgres"
    assert bundle.mcp_details("brave-search")["source_url"] == "https://github.com/modelcontextprotocol/servers-archived/tree/main/src/brave-search"


def test_profile_focus_prefers_non_base_themes() -> None:
    assert _profile_focus(["core", "python", "testing"]) == "python"
    assert _profile_focus(["core", "frontend", "typescript", "mcp", "testing"]) == "frontend, typescript"


def test_profiles_are_ordered_by_focus_then_name() -> None:
    bundle = load_catalog_bundle()

    ordered_names = [name for name, _focus, _themes in _ordered_profiles(bundle)]

    assert ordered_names.index("data-ai") < ordered_names.index("python-core")
    assert ordered_names.index("ts") < ordered_names.index("python-core")


def test_render_themes_lists_theme_rows_alphabetically() -> None:
    bundle = load_catalog_bundle()

    assert render_themes(bundle).columns[0]._cells == sorted(bundle.themes)


def test_render_plugins_lists_plugin_rows_alphabetically() -> None:
    bundle = load_catalog_bundle()

    assert render_plugins(bundle).columns[0]._cells == sorted(bundle.plugins)


def test_render_provider_tables_list_rows_alphabetically() -> None:
    bundle = load_catalog_bundle()

    assert render_providers(bundle, "skill").columns[0]._cells == sorted(bundle.skill_providers)
    assert render_providers(bundle, "agent").columns[0]._cells == sorted(bundle.agent_providers)


def test_target_tree_lists_theme_items_alphabetically() -> None:
    bundle = load_catalog_bundle()

    term = Console(record=True, width=200)
    term.print(build_target_tree(bundle, bundle.resolve_target("docs-design")))
    rendered = term.export_text()

    assert rendered.index("anthropic-algorithmic-art") < rendered.index("anthropic-brand-guidelines")


def test_catalog_source_toml_sections_are_alphabetical() -> None:
    ordered_sections = {
        "themes.toml": ("themes", r'^\[themes\."([^"]+)"\]$'),
        "profiles.toml": ("profiles", r'^\[profiles\."([^"]+)"\]$'),
        "plugins.toml": ("plugins", r'^\[plugins\."([^"]+)"\]$'),
        "repositories.toml": ("repositories", r'^\[repositories\."([^"]+)"\]$'),
        "mcps.toml": ("mcps", r'^\[mcps\."([^"]+)"\]$'),
    }

    for file_name, (_table_name, pattern) in ordered_sections.items():
        text = (CATALOG_DATA_DIR / file_name).read_text(encoding="utf-8")
        names = re.findall(pattern, text, flags=re.MULTILINE)
        assert names == sorted(names)


def test_catalog_source_toml_lists_are_alphabetical() -> None:
    themes = tomllib.loads((CATALOG_DATA_DIR / "themes.toml").read_text(encoding="utf-8"))["themes"]
    profiles = tomllib.loads((CATALOG_DATA_DIR / "profiles.toml").read_text(encoding="utf-8"))["profiles"]

    for theme in themes.values():
        for key in ("plugins", "skills", "agents", "mcps"):
            if key in theme:
                assert theme[key] == sorted(theme[key])

    for profile in profiles.values():
        assert profile["themes"] == sorted(profile["themes"])
