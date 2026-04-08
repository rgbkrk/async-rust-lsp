"""Agent that builds the async-rust-lsp server.

Usage:
    uv run async-rust-lsp-agent "build the LSP server"
    uv run async-rust-lsp-agent  # interactive
"""

from __future__ import annotations

import asyncio
import os
import sys

from claude_agent_sdk import (
    ClaudeAgentOptions,
    ClaudeSDKClient,
    AssistantMessage,
    ResultMessage,
    SystemMessage,
    TextBlock,
)

from agent_template.env import build_clean_env

DEFAULT_MODEL = os.environ.get("AGENT_MODEL", "us.anthropic.claude-sonnet-4-6")

# The agent works in the same repo directory
WORK_DIR = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

SYSTEM_PROMPT = """\
You are a Rust tooling expert building an LSP server called `async-rust-lsp` that provides \
real-time diagnostics for async Rust antipatterns.

## Project Goal
Build a standalone LSP server that detects tokio-specific async antipatterns that clippy and \
rust-analyzer miss. The primary diagnostic is detecting `tokio::sync::Mutex`/`RwLock` guards \
held across `.await` points.

## Context
This was motivated by a real deadlock in the nteract desktop daemon. Clippy's \
`await_holding_lock` only checks `std::sync::Mutex`, not `tokio::sync::Mutex`. No existing \
tool fills this gap with real-time editor feedback.

## Technical Approach
1. Use `tower-lsp` for the LSP framework (async, production-grade)
2. Use `tree-sitter-rust` for incremental Rust parsing (fast, handles partial files)
3. Implement pattern matching rules that detect:
   - tokio Mutex/RwLock guard bindings
   - `.await` expressions within the guard's scope
   - Whether the guard is explicitly dropped before the await
4. Publish diagnostics via `textDocument/publishDiagnostics`
5. Optionally provide code actions (quick fixes)

## Architecture
```
tower-lsp server
  ├── document sync (didOpen/didChange/didSave)
  ├── tree-sitter-rust parser (incremental)
  ├── rule engine
  │   ├── tokio-mutex-across-await (priority 1)
  │   ├── blocking-in-async (priority 2, stretch)
  │   └── (future rules)
  └── diagnostic publisher
```

## Development Steps
1. Set up Cargo project with tower-lsp + tree-sitter dependencies
2. Implement basic LSP lifecycle (initialize, shutdown, didOpen/didChange)
3. Implement tree-sitter parsing on document changes
4. Write the tokio-mutex-across-await detection rule
5. Wire rule results to LSP diagnostics
6. Add tests with sample Rust files
7. Test with a real editor (instructions in README)
8. Write a CLAUDE.md with build/test instructions
9. Commit all work with conventional commits

## Important
- Read the README.md first for full aspirational goals
- Keep the existing Python agent code (src/agent_template/) — it's the agent that drives you
- The Rust LSP code should be at the repo root (Cargo.toml, src/main.rs, src/rules/, etc.)
- The LSP should communicate via stdio (standard for LSP)
- Focus on getting the core diagnostic working first, then polish
"""


async def run_agent(prompt: str):
    """Run the agent on a prompt."""
    clean_env = build_clean_env()

    options = ClaudeAgentOptions(
        system_prompt=SYSTEM_PROMPT,
        model=DEFAULT_MODEL,
        allowed_tools=["Read", "Glob", "Grep", "Bash", "Write", "Edit", "WebSearch", "WebFetch"],
        permission_mode="bypassPermissions",
        max_turns=80,
        cwd=WORK_DIR,
        env=clean_env,
    )

    async with ClaudeSDKClient(options=options) as client:
        await client.query(prompt)
        async for message in client.receive_response():
            if isinstance(message, SystemMessage) and message.subtype == "init":
                model = message.data.get("model", "unknown")
                source = message.data.get("apiKeySource", "unknown")
                print(f"[model={model}, auth={source}]", file=sys.stderr)
            elif isinstance(message, AssistantMessage):
                for block in message.content:
                    if isinstance(block, TextBlock):
                        print(block.text, end="", flush=True)
            elif isinstance(message, ResultMessage):
                print(f"\n\n[{message.num_turns} turns, ${message.total_cost_usd}]",
                      file=sys.stderr)


def main():
    args = sys.argv[1:]
    if args:
        prompt = " ".join(args)
    else:
        prompt = input("prompt> ").strip()
        if not prompt:
            print("No prompt given")
            sys.exit(1)

    asyncio.run(run_agent(prompt))


if __name__ == "__main__":
    main()
