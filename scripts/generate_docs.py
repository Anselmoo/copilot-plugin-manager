from __future__ import annotations

from pathlib import Path

import typer

from copilot_plugin_manager.catalog_docs import write_catalog_docs
from copilot_plugin_manager.rendering import console

ROOT = Path(__file__).resolve().parent.parent
app = typer.Typer(add_completion=False, help="Generate markdown docs derived from the bundled catalog data.")


@app.command()
def main() -> None:
    term = console()
    term.print("[bold]Generating catalog docs[/bold]")
    paths = write_catalog_docs(ROOT)
    for path in paths:
        term.print(f"[green]Wrote[/green] {path.relative_to(ROOT)}")
    term.print(f"[bold green]Done[/bold green] refreshed {len(paths)} markdown targets.")


if __name__ == "__main__":
    app()
