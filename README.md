# Harness

Local model host + coding agent + MCP + file explorer, tuned for models running on your own GPU.
Tauri 2 (Rust) + Preact.

## What it does

- **Engines**: llama.cpp (`llama-server.exe`) for `.gguf` models, PolyStrata for models with a
  `strata-*.json` profile, or any OpenAI-compatible endpoint. One model is loaded at a time.
- **Agent**: pick a workspace in the Files tab. Tools: `read_file` (with outline mode), `list_dir`,
  `glob`, `grep`, `edit_file` (several edits per call, tolerant matching), `edit_lines`, `write_file`,
  `move_file`, `delete_file`, `run_shell`, `update_plan`, plus MCP tools.
  File tools are confined to the workspace; `run_shell` runs in it but is not sandboxed (see Permissions).
- **Checkpoints**: every message is a restore point. Hover a message → Restore checkpoint to put files
  and/or the conversation back to before it (Cursor-style; independent of git). Restores can be undone.
- **Approvals**: file changes are shown as diffs before they're applied. Shell commands can be
  allow-listed by prefix or deny-listed; destructive commands always ask.
- **Project instructions**: `AGENTS.md`, `CLAUDE.md`, `.harness/instructions.md`,
  `.github/copilot-instructions.md` or `.cursorrules` in the workspace are added to the system prompt.
- **Logs tab**: live log (app, agent, engine, MCP), per-run traces (prefill time, KV-cache reuse,
  time to first token, tool timings) and **Evals**: six small agent tasks to compare models and settings.

## Built for local models

- The system prompt and tool list stay byte-identical across turns and `cache_prompt` is sent, so
  llama.cpp reuses its KV cache instead of re-reading the whole conversation every turn.
- Tool output and read windows are sized to the loaded context; near the limit, old tool outputs are
  replaced by stubs in one step (and the decision is saved, so the cache keeps matching).
- Tool calls written as text (`<tool_call>`, `<function=…>`, `[TOOL_CALLS]`, bare JSON) are recovered;
  malformed JSON arguments are repaired and common argument aliases mapped. Repeated identical calls
  are stopped.
- Edits return a short diff to the model instead of the whole file.
- Thinking on/off for reasoning models (Qwen3 etc.) from the composer.

## Data

`%APPDATA%\harness`: `settings.json` (API keys are in Windows Credential Manager), `chats\`,
`checkpoints\`, `logs\` (14 days), `traces\` (last 500 runs), `evals\results.jsonl`, `mcp.json`.

## Develop

```
npm install
npm run tauri dev      # run the app
npm run dev            # UI only, in a browser, with a mock backend and sample data
npm run check          # types, frontend tests, rustfmt, clippy, Rust tests (same as CI)
npm run tauri build    # NSIS installer
```
