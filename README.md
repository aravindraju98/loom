# Harness

Local model host + agent + MCP + file explorer. Tauri 2 (Rust) + Preact.

- **Engines**: PolyStrata (`D:\PolyStrata`, port 8090) for models with a `terminal/strata-*.json` profile; llama.cpp
  (`llama-server.exe`, port 8081) for everything else. One model is loaded at a time. Each model has an engine dropdown
  so you can run the same file on either.
- **Models**: scanned from the folders in Settings (default `E:\PolyStrata-data\models` and the LM Studio folder).
  If a PolyStrata profile points at a moved `.gguf`, a patched copy is written to `%APPDATA%\harness\profiles`.
- **Agent**: choose a workspace folder in the Files tab. Tools: read_file, list_dir, glob, grep (auto) and
  write_file, edit_file, run_shell (ask first). All confined to the workspace.
- **MCP**: edit `%APPDATA%\harness\mcp.json` from the MCP tab (`{"mcpServers": {...}}`, stdio or http).

```
npm install
npm run tauri dev      # run
npm run tauri build    # installer
cd src-tauri && cargo test
```
