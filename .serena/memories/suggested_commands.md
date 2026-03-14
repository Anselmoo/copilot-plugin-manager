Useful project commands on Darwin/macOS:
- ls
- find . -maxdepth 3
- rg <pattern>
- git --no-pager status
- git --no-pager diff
Python/uv workflow currently implied by pyproject.toml:
- uv sync --group dev
- uv run pytest -q
- uv build
Current baseline observation: uv run pytest -q exits with code 5 because no tests are present yet.
Likely future commands after implementation:
- uv run copilot-plugin-manager --help (or chosen console script name)
- uv run pytest -q
- uv build
- twine check dist/* (if twine is added for publish validation)
Legacy reference entrypoint:
- pwsh legacy/copilot-manager.ps1 [command] [argument]
