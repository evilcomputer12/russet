# AI agents

Russet gives coding agents its tools through MCP, the Model Context Protocol. An agent can then build your
app, run it on a device, read its logs, take screenshots and debug it, much as you do in the IDE.

## Russet's two MCP servers

- **Russet** (`russet`) is the engine's server. It runs in WSL and offers the engine's tools: devices,
  builds, runs, debugging, screenshots, logs and signing.
- **Russet IDE** (`russet-ide`) is a read-only look at the IDE itself, for diagnosing Russet: its health
  (the engine connection, the setup steps and the virtual iPhone engine), recent lines of its logs, its user
  settings, and a screenshot of its window.

The editor's own chat always has both servers. Other agents need them registered in your project.

## Install an assistant

1. Run **Russet: Install Agent…**.
2. Pick one or more assistants: Claude Code, Codex, Gemini Code Assist, Continue or Cline. They install from
   Open VSX, the extension gallery Russet uses.
3. The editor asks once per publisher whether you trust it. Accept to install.
4. Russet offers **Register Russet MCP**, which gives the new assistants Russet's tools (see below).

Installing an assistant turns the editor's AI features on, if they were off.

GitHub Copilot is only on Microsoft's marketplace, which Russet's editor cannot use. Copilot's command-line
tool can still use Russet's MCP servers.

## Register Russet's tools for an agent

Registration is per project: the agent gets Russet's tools when it works in that project.

1. Open the project.
2. Run **Russet: Register Russet MCP with…**.
3. Pick the agents. Russet writes their configuration file in the project:

| Agent | File in the project |
| --- | --- |
| Claude Code | `.mcp.json` |
| Codex | `.codex\config.toml` |
| Gemini CLI | `.gemini\settings.json` |
| VS Code agents (Copilot, Continue, Cline) | `.vscode\mcp.json` |
| Cursor | `.cursor\mcp.json` |

Russet adds or replaces only its own two entries, `russet` and `russet-ide`, and keeps everything else in the
file. A JSON file with comments cannot be updated; remove the comments, or add the entries by hand.

To check or share the configuration:

- **Russet: MCP Status** lists the agents registered in the open project, with a **Register…** button.
- **Russet: Copy MCP Config** copies the `mcpServers` JSON for both servers to the clipboard.

The Russet server starts the engine's command line in WSL with the project's path, so the registered files
contain this PC's paths. Register again after you move the project.

## What agents can do

The Russet server's tools fall into these groups:

- **Look**: devices, the project, the doctor's checks, jobs and their logs, the app's logs, screenshots, the
  screen's element tree, crash reports, and the signing and account status.
- **Build and run**: build, clean, install, run, stop and uninstall the app.
- **Debug**: start and stop a debug session, set and remove breakpoints, step, and evaluate expressions.
- **Change your Apple account or publish**: apply a provisioning plan, upload a build, distribute to
  TestFlight.

While Russet is open, breakpoints an agent sets appear in the editor's breakpoint list, and debug sessions it
starts appear in the editor.

Some tools answer `core.notImplemented` in this release, such as touch and text input, setting a location
and uploading a build.

### Approvals

Tools that change your Apple developer account or publish need a person's approval. Their first call fails
with `perm.approvalRequired` and an approval ID, and Russet asks you in a dialog: *The AI agent "…" asks for
your approval: …*, with what it would change. **Approve** lets the agent call the tool again with the approval
ID; **Deny** refuses it. Closing the dialog leaves the request unanswered; it expires after 10 minutes. Approve
only what you asked the agent for. An agent cannot approve its own requests: no command or tool can.

Other permission errors an agent may report are `perm.denied`, `perm.rateLimited` and `perm.tierExceeded`.
The [Error reference](errors.md) explains them.

## Commands that are not available yet

**Russet: Agents**, **Russet: New Agent Session**, **Russet: Ask Local Model…** and
**Russet: Toggle MCP HTTP Endpoint** only say they are coming in a later update. The **Agent Activity** view
in Russet's side bar is empty for now.
