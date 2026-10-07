"""A small autonomous agent, the starting point `agent init` puts in ~/app.

Every AGENT_INTERVAL seconds it reads TASK.md (what to do) and MEMORY.md (what it noted
before), works on the task with a shell tool, and keeps notes for the next run by writing to
MEMORY.md. Edit TASK.md to give it a job; edit this file to change how it thinks.

One loop, several model providers, picked by AGENT_PROVIDER:
  anthropic   Claude, via the anthropic SDK              ANTHROPIC_API_KEY
  openai      OpenAI, via the openai SDK                 OPENAI_API_KEY
  gemini      Google Gemini, via the google-genai SDK    GEMINI_API_KEY
  xai | deepseek | openrouter | mistral
              OpenAI-compatible APIs, via the openai SDK  XAI_API_KEY, DEEPSEEK_API_KEY, ...
MODEL overrides the model. Run `python3 agent.py --once` to try a single run.
"""

import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

APP = Path(__file__).resolve().parent
PROVIDER = os.environ.get("AGENT_PROVIDER", "openai")
INTERVAL = int(os.environ.get("AGENT_INTERVAL", "600"))
MAX_STEPS = int(os.environ.get("AGENT_MAX_STEPS", "25"))

DEFAULT_MODELS = {
    "anthropic": "claude-opus-5-5",
    "openai": "gpt-5.4-mini",
    "gemini": "gemini-2.5-flash",
    "xai": "grok-4",
    "deepseek": "deepseek-chat",
    "openrouter": "openai/gpt-5.4-mini",
    "mistral": "mistral-medium-latest",
}
COMPATIBLE = {
    "xai": ("https://api.x.ai/v1", "XAI_API_KEY"),
    "deepseek": ("https://api.deepseek.com", "DEEPSEEK_API_KEY"),
    "openrouter": ("https://openrouter.ai/api/v1", "OPENROUTER_API_KEY"),
    "mistral": ("https://api.mistral.ai/v1", "MISTRAL_API_KEY"),
}
MODEL = os.environ.get("MODEL") or DEFAULT_MODELS.get(PROVIDER, "")

SYSTEM = """You are an autonomous agent running in your own Linux sandbox inside a confidential
TEE. You act through the `shell` tool (bash, as an unprivileged user, with internet access,
Python 3, Node.js, git and curl). Your working directory is {app}.

- TASK.md says what to do. MEMORY.md holds your notes from earlier runs.
- Before you finish, append anything worth remembering to MEMORY.md (e.g. `cat >> MEMORY.md`).
- Secrets are environment variables loaded from ~/.env. Never print or log their values.
- Your own Celestia wallet mnemonic is ~/.agentcloud/wallet.mnemonic; it pays your rent, and
  ~/.agentcloud/lease.json says how much and how to check your lease.
- Be economical: every run costs money. Finish with a one-paragraph summary of what you did."""


def shell(command: str) -> str:
    """Run a bash command in the agent's app directory and return its output.

    Args:
        command: The bash command to run.
    """
    try:
        done = subprocess.run(
            ["bash", "-lc", command], cwd=APP, capture_output=True, text=True, timeout=180
        )
        output = (done.stdout + done.stderr).strip() or "(no output)"
        return f"exit {done.returncode}\n{output[-12000:]}"
    except subprocess.TimeoutExpired:
        return "error: the command took longer than 180 seconds and was stopped"


SHELL_SCHEMA = {
    "type": "object",
    "properties": {"command": {"type": "string", "description": "The bash command to run."}},
    "required": ["command"],
}


def prompt() -> str:
    task = (APP / "TASK.md").read_text() if (APP / "TASK.md").exists() else "No task yet."
    memory = (APP / "MEMORY.md").read_text()[-8000:] if (APP / "MEMORY.md").exists() else "(empty)"
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")
    return f"Time: {now}\n\n# TASK.md\n{task}\n\n# MEMORY.md (latest)\n{memory}"


def run_anthropic() -> str:
    import anthropic
    from anthropic import beta_tool

    runner = anthropic.Anthropic().beta.messages.tool_runner(
        model=MODEL,
        max_tokens=16000,
        system=SYSTEM.format(app=APP),
        tools=[beta_tool(shell)],
        messages=[{"role": "user", "content": prompt()}],
    )
    final = None
    for message in runner:
        final = message
        for block in message.content:
            if block.type == "tool_use":
                print(f"$ {block.input.get('command', '')}", flush=True)
    if final is None:
        return "(no response)"
    if final.stop_reason == "refusal":
        return "(the model declined this run)"
    return "".join(block.text for block in final.content if block.type == "text")


def run_openai() -> str:
    import json

    from openai import OpenAI

    if PROVIDER in COMPATIBLE:
        base_url, key_var = COMPATIBLE[PROVIDER]
        client = OpenAI(base_url=base_url, api_key=os.environ[key_var])
    else:
        client = OpenAI()
    tools = [{"type": "function", "function": {"name": "shell", "description": shell.__doc__, "parameters": SHELL_SCHEMA}}]
    messages = [{"role": "system", "content": SYSTEM.format(app=APP)}, {"role": "user", "content": prompt()}]
    for _ in range(MAX_STEPS):
        reply = client.chat.completions.create(model=MODEL, messages=messages, tools=tools).choices[0].message
        messages.append(reply.model_dump(exclude_none=True))
        if not reply.tool_calls:
            return reply.content or ""
        for call in reply.tool_calls:
            command = json.loads(call.function.arguments).get("command", "")
            print(f"$ {command}", flush=True)
            messages.append({"role": "tool", "tool_call_id": call.id, "content": shell(command)})
    return "(stopped after the step limit)"


def run_gemini() -> str:
    from google import genai
    from google.genai import types

    config = types.GenerateContentConfig(
        system_instruction=SYSTEM.format(app=APP),
        tools=[shell],
        automatic_function_calling=types.AutomaticFunctionCallingConfig(maximum_remote_calls=MAX_STEPS),
    )
    return genai.Client().models.generate_content(model=MODEL, contents=prompt(), config=config).text or ""


def run_once() -> None:
    backend = {"anthropic": run_anthropic, "gemini": run_gemini}.get(PROVIDER, run_openai)
    print(f"--- run at {datetime.now(timezone.utc):%Y-%m-%d %H:%M:%S} UTC with {PROVIDER} {MODEL}", flush=True)
    try:
        print(backend(), flush=True)
    except Exception as error:  # A failed run is logged; the next one tries again.
        print(f"run failed: {type(error).__name__}: {error}", flush=True)


if __name__ == "__main__":
    if "--once" in sys.argv:
        run_once()
        sys.exit(0)
    while True:
        run_once()
        time.sleep(INTERVAL)
