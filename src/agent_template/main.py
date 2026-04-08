"""Example agent using Claude Agent SDK with Bedrock auth.

Usage:
    uv run my-agent "your prompt here"
    uv run my-agent                      # interactive
"""

from __future__ import annotations

import asyncio
import os
import sys
import tempfile

from claude_agent_sdk import (
    ClaudeAgentOptions,
    ClaudeSDKClient,
    AssistantMessage,
    ResultMessage,
    SystemMessage,
    TextBlock,
)

from agent_template.env import build_clean_env

# Default to Sonnet 4.6 on Bedrock. Override with AGENT_MODEL env var.
DEFAULT_MODEL = os.environ.get("AGENT_MODEL", "us.anthropic.claude-sonnet-4-6")

SYSTEM_PROMPT = """\
You are a helpful assistant.
"""


async def run_agent(prompt: str):
    """Run the agent on a prompt."""
    clean_env = build_clean_env()

    work_dir = tempfile.mkdtemp(prefix="agent-")

    options = ClaudeAgentOptions(
        system_prompt=SYSTEM_PROMPT,
        model=DEFAULT_MODEL,
        allowed_tools=["Read", "Glob", "Grep", "Bash", "Write", "Edit"],
        permission_mode="bypassPermissions",
        max_turns=20,
        cwd=work_dir,
        env=clean_env,
        # Add MCP servers here:
        # mcp_servers={
        #     "my-server": {"command": "path/to/server"},
        # },
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
