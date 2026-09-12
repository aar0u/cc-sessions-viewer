# Tool management

**English** · [中文](README.zh-CN.md) · [日本語](README.ja.md)

Every agent CLI keeps its own skills, MCP servers, hooks, and instruction files somewhere on disk. Install a few tools, switch between agents for a few months, and nobody knows what is actually loaded anymore — the same skill exists in three folders, a link points at something that was deleted, and a server you configured once is quietly running in an agent you forgot about.

This page shows you all of it in one place, and lets you fix it.

**Open it** from the wrench icon at the bottom of the sidebar, or press `⌘K`.

---

## Skills

![Skills panel](../screenshots/tools-skills.png)

The bar across the top is the summary for this machine: 45 skills, **28 duplicated**, **12 reached through a detour**, **1 pointing at nothing**. Click any of those to filter the list down to just those skills.

Pick a skill and the right side tells you everything that matters before you touch it:

- **Enabled in** — which agents can actually see it. Toggle an agent on or off here.
- **Content** — where the actual files live. There can be more than one copy.
- **References** — every link pointing at it, and what it resolves through.
- **Risk findings** — shell commands found in the skill, with the risky-looking ones called out. A `rm -rf` used as an example inside a code block is scored lower than the same line in an executable script, so the badge still means something.

Sort by **newest / oldest / name**; pinned skills stay on top regardless.

### Move to main store

![Move to main store](../screenshots/tools-skills-adopt.png)

Skills scattered across different folders are the root of most of the mess. This gathers one into your main store and leaves a link behind, so nothing stops working.

You see the exact steps before anything happens — here, one `move` and one `link`. Nothing is written until you press the button. **Move all to main store** at the top does the whole list at once.

### Repair links

![Repair links](../screenshots/tools-skills-repair.png)

A **detour** is a link that points at another link. It works until one day it doesn't, and then it is very hard to figure out why the skill disappeared.

Repair points the link straight at the real folder. In this example `~/.claude/skills/three` was going through `~/.skills-manager` to reach `~/.cc-switch` — afterwards it goes there directly.

### Delete

![Delete](../screenshots/tools-skills-delete.png)

This is where dead links come from: something deletes a skill's folder and leaves every link to it dangling.

Deleting here **unlinks every reference first, then removes every copy** — and shows you the complete list before it starts. Here one skill turned out to live in three separate stores at once. You can also remove a single copy from the **Content** list if you want to keep the project-level one and drop the global one.

---

## MCP

![MCP panel](../screenshots/tools-mcp.png)

Every MCP server across all seven agents, in one list, whether it was written in JSON or TOML.

- **Context budget** — how much of your context window your servers eat before you type anything. One server here is 29 tools and about 5,700 tokens.
- **Active in** — which agents run this server, and which file says so. Notice **Grok Build · Read for compatibility**: Grok reads Claude's config by default, so a server you added to Claude is running there too. That is normally invisible.
- Add, edit, remove, enable/disable, or copy a server to other agents — each one shows you the file changes first.
- Anything that looks like a token or key is masked until you ask to see it.

---

## Discover skills

![Discover panel](../screenshots/tools-discover.png)

Search [skills.sh](https://www.skills.sh) and read the skill **before** installing it — description, file list, which commit, where it sits in the repo.

**Copy and install** opens a terminal right there and types the command in. You watch it run and can stop it with `Ctrl-C`. It pauses at its own "which agents?" prompt and waits for you — the app does not answer that for you.

---

## Hooks

![Hooks panel](../screenshots/tools-hooks.png)

Hooks are grouped by command, not by file. The first entry here is one script wired into 4 agents across 9 events — as one row instead of twenty-one.

- **Dry run** a hook with a real payload and see its output, exit code, and how long it took.
- Remove a whole hook or just one place it is wired in.
- Hooks this app installed itself are marked and protected from accidental removal.

---

## Global config

![Global config panel](../screenshots/tools-memo.png)

`CLAUDE.md`, `AGENTS.md`, and everything they pull in.

- **falls back** — opencode has no file of its own, so it reads Claude's. Editing that file changes both.
- **not created** — this agent supports one, you just don't have it yet.
- **fragment** — a file pulled in via `@import` by one of the others.
- **also read** — a file outside the usual path that an agent loads anyway.

Same-named files that have drifted apart get flagged, with a side-by-side diff and a button to sync one over the other.

---

## Config bundle

The archive icon in the top bar exports your setup as a shareable file. **Values are never included** — a teammate gets the shape of your config, not your API keys. On import you choose which agents receive each entry.

---

## Two things that always hold

**Nothing is written until you confirm it.** Every action shows the exact file changes first, and you can cancel.

**Existing files are preserved.** Only the keys this app owns are rewritten, the original is backed up alongside, and if the file changed on disk since it was read, the write is refused rather than applied over your edit.
