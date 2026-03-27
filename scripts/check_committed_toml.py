from __future__ import annotations

import sys
import tomllib
from pathlib import Path

type TomlValue = dict[str, "TomlValue"] | list["TomlValue"] | str | int | float | bool | None


REPO_ROOT = Path(__file__).resolve().parents[1]
MANIFESTS = ("cpm.toml", "cpm-reference.toml")
ASSET_SECTIONS = {"plugins", "skills", "agents", "mcps", "hooks", "workflows", "instructions"}


def _find_absolute_paths(value: TomlValue, location: str) -> list[str]:
    errors: list[str] = []
    if isinstance(value, dict):
        path_value = value.get("path")
        if isinstance(path_value, str) and Path(path_value).is_absolute():
            errors.append(f"{location}.path must be repo-relative, found {path_value!r}")
        for key, item in value.items():
            child_location = f"{location}.{key}" if location else key
            errors.extend(_find_absolute_paths(item, child_location))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            errors.extend(_find_absolute_paths(item, f"{location}[{index}]"))
    return errors


def _find_legacy_group_usage(value: TomlValue, location: str) -> list[str]:
    errors: list[str] = []
    if isinstance(value, dict):
        for key, item in value.items():
            child_location = f"{location}.{key}" if location else key
            if key == "group":
                errors.append(f'{child_location} uses legacy `group`; prefer `groups = ["..."]`')
            if location.startswith("groups.") and key in ASSET_SECTIONS:
                message = (
                    f"{child_location} is a legacy nested group asset section; "
                    "keep `[groups.<name>]` for metadata only and move assets "
                    "inline with `groups = [...]`"
                )
                errors.append(message)
            errors.extend(_find_legacy_group_usage(item, child_location))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            errors.extend(_find_legacy_group_usage(item, f"{location}[{index}]"))
    return errors


def main() -> int:
    errors: list[str] = []

    for manifest_name in MANIFESTS:
        manifest_path = REPO_ROOT / manifest_name
        try:
            parsed = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError as exc:
            errors.append(f"{manifest_name}: invalid TOML ({exc})")
            continue

        errors.extend(f"{manifest_name}: {error}" for error in _find_absolute_paths(parsed, ""))
        errors.extend(f"{manifest_name}: {error}" for error in _find_legacy_group_usage(parsed, ""))

    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    for manifest_name in MANIFESTS:
        print(f"validated {manifest_name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
