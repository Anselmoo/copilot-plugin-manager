from __future__ import annotations

import sys

from ._cli import invoke_delegate


def main() -> None:
    args = sys.argv[1:]
    prefer_cargo = False

    if "--cargo" in args:
        prefer_cargo = True
        args = [arg for arg in args if arg != "--cargo"]

    raise SystemExit(invoke_delegate(args, prefer_cargo=prefer_cargo))


if __name__ == "__main__":
    main()
