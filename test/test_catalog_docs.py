from pathlib import Path

from copilot_plugin_manager.catalog import load_catalog_bundle
from copilot_plugin_manager.catalog_docs import (
    README_SECTION_END,
    README_SECTION_START,
    render_credits_markdown,
    render_readme_section,
    render_themes_markdown,
    update_readme_generated_section,
    write_catalog_docs,
)


def test_render_readme_section_includes_profile_overview() -> None:
    section = render_readme_section(load_catalog_bundle())

    assert "uv run poe generate-docs" in section
    assert "| `python-mcp` |" in section
    assert "| `ts-mcp` |" in section
    assert "docs/THEMES.md" in section
    assert "docs/CREDITS.md" in section


def test_render_generated_docs_include_expected_sections() -> None:
    bundle = load_catalog_bundle()

    themes_doc = render_themes_markdown(bundle)
    credits_doc = render_credits_markdown(bundle)

    assert "# Themes and profile compositions" in themes_doc
    assert "## Profiles" in themes_doc
    assert "### `core`" in themes_doc
    assert "# Credits" in credits_doc
    assert "| `awesome-copilot` |" in credits_doc


def test_update_readme_generated_section_replaces_marker_block() -> None:
    bundle = load_catalog_bundle()
    readme = "\n".join(
        [
            "# Demo",
            "",
            README_SECTION_START,
            "old content",
            README_SECTION_END,
            "",
        ]
    )

    updated = update_readme_generated_section(readme, bundle)

    assert "old content" not in updated
    assert "Current profile compositions" in updated


def test_write_catalog_docs_writes_markdown_files(tmp_path: Path) -> None:
    (tmp_path / "README.md").write_text(
        "\n".join(
            [
                "# Demo",
                "",
                README_SECTION_START,
                "placeholder",
                README_SECTION_END,
                "",
            ]
        ),
        encoding="utf-8",
    )

    paths = write_catalog_docs(tmp_path, load_catalog_bundle())

    assert (tmp_path / "docs" / "CREDITS.md").exists()
    assert (tmp_path / "docs" / "THEMES.md").exists()
    assert (tmp_path / "README.md").read_text(encoding="utf-8").count("Current profile compositions") == 1
    assert {path.name for path in paths} == {"CREDITS.md", "THEMES.md", "README.md"}
