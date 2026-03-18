from __future__ import annotations

import os
from pathlib import Path

import click
from click.shell_completion import CompletionItem, ShellComplete, add_completion_class, get_completion_class, split_arg_string
from typer._completion_classes import completion_init

COMPLETE_ENV_VAR = "_COPILOT_PLUGIN_MANAGER_COMPLETE"
DEFAULT_COMMAND_NAME = "copilot-plugin-manager"

completion_init()

_SOURCE_NUSHELL = """def %(complete_func)s [...spans: string] {
    let incomplete = if ($spans | is-empty) { "" } else { $spans | last }
    let response = (with-env {
        %(complete_var)s: "nushell_complete"
        COMP_WORDS: ($spans | str join " ")
        COMP_CWORD: $incomplete
    } {
        ^%(prog_name)s | lines
    })

    $response | each {|line|
        let parts = ($line | split row (char tab))
        if (($parts | length) > 1) {
            {value: ($parts | get 0), description: ($parts | get 1)}
        } else {
            ($parts | get 0)
        }
    }
}

extern "%(prog_name)s" [
    ...args: string@%(complete_func)s
]
"""


class _EnvWordListComplete(ShellComplete):
    def get_completion_args(self) -> tuple[list[str], str]:
        cwords = split_arg_string(os.environ.get("COMP_WORDS", ""))
        incomplete = os.environ.get("COMP_CWORD", "")
        args = cwords[1:]
        if incomplete and args and args[-1] == incomplete:
            args.pop()
        return args, incomplete


class NushellComplete(_EnvWordListComplete):
    name = "nushell"
    source_template = _SOURCE_NUSHELL

    def format_completion(self, item: CompletionItem) -> str:
        return f"{item.value}\t{item.help or item.value}"


add_completion_class(NushellComplete)


def completion_source(
    cli: click.Command,
    shell: str,
    command_name: str = DEFAULT_COMMAND_NAME,
    complete_var: str = COMPLETE_ENV_VAR,
) -> str:
    completion_class = get_completion_class(shell)
    if completion_class is None:
        raise KeyError(shell)
    return completion_class(cli, {}, command_name, complete_var).source()


def shell_init_snippet(shell: str, command_name: str = DEFAULT_COMMAND_NAME) -> str:
    snippets = {
        "bash": f'eval "$({command_name} completion script bash)"',
        "zsh": f'eval "$({command_name} completion script zsh)"',
        "fish": f"{command_name} completion script fish | source",
        "powershell": f"& {{ {command_name} completion script powershell }} | Invoke-Expression",
        "nushell": f"# Run `{command_name} completion install nushell` once, then add this to config.nu\nsource {default_completion_path('nushell', command_name)}",
    }
    return snippets[shell]


def default_completion_path(shell: str, command_name: str = DEFAULT_COMMAND_NAME) -> Path:
    file_name = _completion_file_name(shell, command_name)
    if shell == "bash":
        return _xdg_data_home() / "bash-completion" / "completions" / file_name
    if shell == "zsh":
        return Path.home() / ".zfunc" / file_name
    if shell == "fish":
        return _xdg_config_home() / "fish" / "completions" / file_name
    if shell == "powershell":
        return _powershell_config_home() / "Completions" / file_name
    if shell == "nushell":
        return _nushell_config_home() / "completions" / file_name
    raise KeyError(shell)


def install_completion_script(
    cli: click.Command,
    shell: str,
    path: Path | None = None,
    command_name: str = DEFAULT_COMMAND_NAME,
) -> Path:
    target = (path or default_completion_path(shell, command_name)).expanduser()
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(completion_source(cli, shell, command_name), encoding="utf-8")
    return target


def install_completion_message(shell: str, target: Path) -> str | None:
    if shell == "bash":
        return f"Most bash-completion setups load this path automatically.\nIf yours does not, add `source {target}` to your shell startup."
    if shell == "zsh":
        return f"If this directory is not already on your `fpath`, add the following to `~/.zshrc`:\nfpath+=({target.parent})\nautoload -Uz compinit && compinit"
    if shell == "fish":
        return "Fish auto-loads completions from this path."
    if shell == "powershell":
        return f"Add this to `$PROFILE` to load the installed script:\n. '{target}'"
    if shell == "nushell":
        return f"Add this to `config.nu` to load the installed script:\nsource {target}"
    return None


def _completion_file_name(shell: str, command_name: str) -> str:
    if shell == "zsh":
        return f"_{command_name}"
    extensions = {
        "bash": "",
        "fish": ".fish",
        "powershell": ".ps1",
        "nushell": ".nu",
    }
    return f"{command_name}{extensions.get(shell, '')}"


def _xdg_config_home() -> Path:
    return Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")).expanduser()


def _xdg_data_home() -> Path:
    return Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")).expanduser()


def _powershell_config_home() -> Path:
    if os.name == "nt":
        return Path(os.environ.get("USERPROFILE", str(Path.home()))).expanduser() / "Documents" / "PowerShell"
    return _xdg_config_home() / "powershell"


def _nushell_config_home() -> Path:
    if os.name == "nt":
        appdata = Path(os.environ.get("APPDATA", str(Path.home()))).expanduser()
        return appdata / "nushell"
    return _xdg_config_home() / "nushell"
