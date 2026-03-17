from copilot_plugin_manager.catalog import load_catalog_bundle


def test_catalog_bundle_counts() -> None:
    bundle = load_catalog_bundle()
    assert len(bundle.plugins) > 0
    assert len(bundle.repositories) == 7
    assert len(bundle.skill_providers) == 210
    assert len(bundle.agent_providers) == 58
    assert len(bundle.entrypoints) > len(bundle.plugins)
    assert len(bundle.themes) == 29
    assert len(bundle.profiles) == 38


def test_resolve_known_profile() -> None:
    bundle = load_catalog_bundle()
    target = bundle.resolve_target("python-mcp")
    assert target.kind == "profile"
    assert target.themes == ["core", "python", "mcp", "testing", "python-agents", "mcp-agents"]


def test_resolve_added_curated_profiles() -> None:
    bundle = load_catalog_bundle()

    assert bundle.resolve_target("ts").themes == ["core", "frontend", "typescript", "testing"]
    assert bundle.resolve_target("ts-mcp").themes == ["core", "frontend", "typescript", "mcp", "testing", "mcp-agents"]
    assert bundle.resolve_target("python-plus-rust").themes == ["core", "python", "rust", "data", "testing"]
    assert bundle.resolve_target("pydantic").themes == ["core", "python", "openapi", "testing"]
    assert bundle.resolve_target("fastapi-typer").themes == ["core", "python", "openapi", "testing"]
    assert bundle.resolve_target("backend").themes == bundle.resolve_target("backend-api").themes
    assert bundle.resolve_target("scientific-programming").themes == ["core", "science", "python", "data", "research"]


def test_rust_theme_exposes_rust_specific_content() -> None:
    bundle = load_catalog_bundle()
    rust_theme = bundle.themes["rust"]

    assert "rust-mcp-development" in rust_theme.plugins
    assert "mskills-rust" in rust_theme.skills


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
