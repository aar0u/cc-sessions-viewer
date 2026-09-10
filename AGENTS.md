# AGENTS.md

## Verifying the running UI

When a task needs you to **look at or drive the actual running app** — take a
screenshot, read the live DOM, click a real button, or call a real Tauri command
against real data — read this first and follow it:

**[`.claude/skills/tauri-dev-mcp/SKILL.md`](.claude/skills/tauri-dev-mcp/SKILL.md)**

Read it before you start, not after something fails. Launching the dev build the
obvious way (`npm run tauri dev`) compiles **without** the MCP bridge, and every
bridge tool then fails to connect with nothing in the log saying why. The file
also covers the port-1420 conflict, why screenshots come back stale when the
window is not frontmost, and how to shut down without leaving processes behind.
