# Releasing and publishing

## CI/CD workflow

The repository uses [`cicd.yml`](../.github/workflows/cicd.yml) for validation, building, and publishing.

That workflow currently does the following:

- runs Ruff, `ty`, and the test suite on pushes and pull requests
- builds source and wheel distributions on non-PR pushes
- publishes to TestPyPI for pushes to `main`
- publishes to PyPI for tags that match `v*.*.*`

Workflow page:

- <https://github.com/Anselmoo/copilot-plugin-manager/actions/workflows/cicd.yml>

Package page:

- <https://pypi.org/project/copilot-plugin-manager/>

## Local pre-release checks

Before creating a release tag, run:

```bash
uv run poe check
uv run poe build
```

## Publishing flow

1. Update version metadata as needed.
2. Run the local validation commands.
3. Create and push a version tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Pushing a matching version tag triggers the `publish-pypi` job in `cicd.yml`.

## TestPyPI

Pushes to `main` trigger the `publish-testpypi` job so packaging can be validated before a tagged release.
