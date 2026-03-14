from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path


def load_refresh_catalog_module():
    module_path = Path(__file__).resolve().parents[1] / "scripts" / "refresh_catalog.py"
    spec = spec_from_file_location("refresh_catalog", module_path)
    assert spec is not None and spec.loader is not None
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_load_toml_returns_empty_dict_for_empty_files(tmp_path: Path) -> None:
    refresh_catalog = load_refresh_catalog_module()
    empty = tmp_path / "empty.toml"
    empty.write_text("")

    assert refresh_catalog.load_toml(empty) == {}


def test_bootstrap_provider_catalog_infers_expected_roots() -> None:
    refresh_catalog = load_refresh_catalog_module()
    repositories = {
        "microsoft-skills": {
            "owner": "microsoft",
            "repo": "skills",
            "url": "https://github.com/microsoft/skills",
            "version_channel": "main",
        },
        "anthropics-skills": {
            "owner": "anthropics",
            "repo": "skills",
            "url": "https://github.com/anthropics/skills",
            "version_channel": "main",
        },
        "agency-agents": {
            "owner": "msitarzewski",
            "repo": "agency-agents",
            "url": "https://github.com/msitarzewski/agency-agents",
            "version_channel": "main",
        },
        "voltagent-subagents": {
            "owner": "VoltAgent",
            "repo": "awesome-claude-code-subagents",
            "url": "https://github.com/VoltAgent/awesome-claude-code-subagents",
            "version_channel": "main",
        },
    }
    previous = {
        "skill:mskills-python-data:skills/python/data": {
            "kind": "skill",
            "provider": "mskills-python-data",
            "source": "microsoft-skills",
            "source_path": "skills/python/data",
            "source_url": "https://github.com/microsoft/skills/tree/main/skills/python/data",
            "tags": ["skill", "python", "data"],
        },
        "skill:anthropic-pdf:skills/pdf/scripts": {
            "kind": "skill",
            "provider": "anthropic-pdf",
            "source": "anthropics-skills",
            "source_path": "skills/pdf/scripts",
            "source_url": "https://github.com/anthropics/skills/tree/main/skills/pdf/scripts",
            "tags": ["skill", "docs"],
        },
        "agent:agency-design-brand-guardian:design/design-brand-guardian.md": {
            "kind": "agent",
            "provider": "agency-design-brand-guardian",
            "source": "agency-agents",
            "source_path": "design/design-brand-guardian.md",
            "source_url": "https://github.com/msitarzewski/agency-agents/blob/main/design/design-brand-guardian.md",
            "tags": ["agent", "agency"],
        },
        "agent:voltagent:categories/01-core-development/api-designer.md": {
            "kind": "agent",
            "provider": "voltagent",
            "source": "voltagent-subagents",
            "source_path": "categories/01-core-development/api-designer.md",
            "source_url": "https://github.com/VoltAgent/awesome-claude-code-subagents/blob/main/categories/01-core-development/api-designer.md",
            "tags": ["agent", "voltagent"],
        },
        "agent:voltagent:categories/02-language-specialists/python-pro.md": {
            "kind": "agent",
            "provider": "voltagent",
            "source": "voltagent-subagents",
            "source_path": "categories/02-language-specialists/python-pro.md",
            "source_url": "https://github.com/VoltAgent/awesome-claude-code-subagents/blob/main/categories/02-language-specialists/python-pro.md",
            "tags": ["agent", "voltagent"],
        },
    }

    skill_providers = refresh_catalog.bootstrap_provider_catalog("skill", repositories, previous)
    agent_providers = refresh_catalog.bootstrap_provider_catalog("agent", repositories, previous)

    assert skill_providers["mskills-python-data"]["roots"] == ["skills/python/data"]
    assert skill_providers["anthropic-pdf"]["roots"] == ["skills/pdf"]
    assert agent_providers["agency-design-brand-guardian"]["roots"] == ["design/design-brand-guardian.md"]
    assert agent_providers["voltagent"]["roots"] == [
        "categories/01-core-development",
        "categories/02-language-specialists",
    ]


def test_build_provider_records_makes_single_skill_metadata_specific() -> None:
    refresh_catalog = load_refresh_catalog_module()
    repositories = {
        "microsoft-skills": {
            "owner": "microsoft",
            "repo": "skills",
            "url": "https://github.com/microsoft/skills",
            "version_channel": "main",
        }
    }
    providers = {
        "mskills-python-data": {
            "source": "microsoft-skills",
            "prefix": "mskills-python-data",
            "roots": ["skills/python/data"],
            "homepage": "https://github.com/microsoft/skills/tree/main/skills/python/data",
            "version_channel": "main",
            "tags": ["skill", "python", "data", "microsoft"],
        }
    }
    entrypoints = [
        {
            "provider": "mskills-python-data",
            "title": "Data",
            "description": "Data",
            "source_url": "https://github.com/microsoft/skills/tree/main/skills/python/data",
            "tags": ["skill", "python", "data", "microsoft"],
        }
    ]

    records = refresh_catalog.build_provider_records("skill", providers, repositories, entrypoints)

    assert records["mskills-python-data"]["description"] == "Microsoft Python Data skill pack synced into the local skills catalog."
    assert records["mskills-python-data"]["use_when"] == "Use when you want the Microsoft Python Data skill pack available locally."


def test_build_plugin_records_infers_specific_use_when_without_existing_catalog(tmp_path: Path, monkeypatch) -> None:
    refresh_catalog = load_refresh_catalog_module()
    monkeypatch.setattr(refresh_catalog, "ROOT", tmp_path)
    plugin_root = tmp_path / "external" / "awesome-copilot" / "plugins" / "debug-helper"
    plugin_root.mkdir(parents=True)
    (plugin_root / "README.md").write_text("# Debug Helper\n\nToolkit for debugging API integrations and tracing failures.\n")

    records, _ = refresh_catalog.build_plugin_records({}, {})

    assert records["debug-helper"]["use_when"] == "Use when you need toolkit for debugging API integrations and tracing failures."
