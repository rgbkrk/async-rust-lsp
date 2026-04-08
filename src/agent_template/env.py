"""Clean environment builder for Claude Agent SDK.

Builds a minimal env (like env -i) so no session state leaks into
the agent subprocess. Auth credentials come from .env file or
explicit environment variables.
"""

from __future__ import annotations

import os
from pathlib import Path


def load_dotenv(path: Path | None = None) -> dict[str, str]:
    """Load a .env file into a dict. Skips comments and blank lines."""
    if path is None:
        path = Path(__file__).resolve().parent.parent.parent / ".env"
    if not path.exists():
        return {}
    env = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        key, _, value = line.partition("=")
        value = value.strip().strip("'\"")
        env[key.strip()] = value
    return env


def build_clean_env(dotenv_path: Path | None = None) -> dict[str, str]:
    """Build a minimal clean environment for the agent subprocess.

    Like `env -i` — only includes what's explicitly needed.
    Loads .env file for auth credentials, then overlays any matching
    vars from the current environment (so CLI overrides work).
    """
    home = os.path.expanduser("~")

    clean = {
        "HOME": home,
        "USER": os.environ.get("USER", ""),
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "SHELL": os.environ.get("SHELL", "/bin/zsh"),
        "TERM": os.environ.get("TERM", "xterm-256color"),
    }

    dotenv = load_dotenv(dotenv_path)

    # Auth vars — from .env, overridden by current env if set
    auth_keys = [
        "AWS_BEARER_TOKEN_BEDROCK",
        "AWS_REGION",
        "AWS_PROFILE",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BEDROCK_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
    ]
    for key in auth_keys:
        val = os.environ.get(key, dotenv.get(key))
        if val:
            clean[key] = val

    return clean
