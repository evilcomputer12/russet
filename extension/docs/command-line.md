# Command line

`russet` is Russet's command line. It runs in any terminal (PowerShell, Command Prompt, Git Bash) and in
scripts, and AI agents use it the way they use `xcodebuild` or `flutter`. Russet's window does not need to be
open: `russet` starts Russet's engine in WSL itself, with your sign-in and your iPhone.

The installer puts `russet` on your PATH. Terminals that were open before the install do not see it yet: open
a new one. Russet's own terminals see it at once.

```
russet doctor
russet device list
russet build
russet run
```

## Windows paths

Write paths as Windows paths, as you would for any Windows program: `C:\Users\you\MyApp`, or relative to the
folder you are in. What `russet` prints has Windows paths too. In `--json` output they use forward slashes
(`C:/Users/you/MyApp`), which JSON needs no escaping for and Windows accepts.

## What it can do

Everything the engine does, with the engine's own commands. A few:

| Command | What it does |
| --- | --- |
| `russet doctor` | Checks Swift, the iOS SDK, your account and the iPhone, and says what to fix. |
| `russet device list` | Lists the iPhones connected through Apple Devices. |
| `russet build` | Builds the project in the current folder (or `--project PATH`). |
| `russet run [--device UDID]` | Builds, signs, installs and launches the app on the iPhone. |
| `russet debug [--break FILE:LINE] [--batch]` | The same, then debugs it with LLDB in the terminal. |
| `russet logs --app BUNDLE_ID --follow` | Streams the app's log from the iPhone. |
| `russet screen screenshot --output shot.png` | Saves a screenshot of the iPhone. |
| `russet provision` | Lists the changes your Apple account needs for this app and iPhone, and makes them once you agree. |
| `russet approvals list`, `russet approve ID` | Lists and grants approvals (in a terminal). |
| `russet mcp serve --client NAME` | Russet's MCP server over standard input and output, for an AI agent. |

`russet --help` lists every command; `russet COMMAND --help` explains one. Add `--json` for output a program
reads: one JSON document, or one per line for a stream.

What only Windows has:

| Command | What it does |
| --- | --- |
| `russet vm status` | Whether the virtual iPhone engine is built, a virtual iPhone is created, and one runs. |
| `russet vm create [--device D] [--ios V]` | Creates a virtual iPhone (hours on its first boot; it carries on in the background). |
| `russet vm start [--device D --ios V]`, `russet vm stop` | Starts or stops it. |
| `russet vm unlock`, `russet vm button lock\|home\|volume-up\|volume-down` | Unlocks it, presses a button. |
| `russet vm install APP` | Installs and launches an `.app` folder or `.ipa` on it. |
| `russet vm debug APP [LLDB ARGUMENTS]` | Installs it, launches it stopped and debugs it with LLDB in the terminal. |
| `russet setup status` | The setup steps and their state, as on the setup page. |
| `russet setup run STEP [--repair] [--xip PATH]` | Runs one step: `backend`, `swift`, `xtool`, `xcode-sdk` (with `--xip`), `vphone`, `virtual-iphone`. |

`russet vm debug` passes the arguments after the app to LLDB, so a script can debug without a person:

```
russet vm debug path\to\MyApp.app -b -o "breakpoint set -n main" -o continue -o bt
```

## Your iPhone

`russet` reaches an iPhone through Apple Devices, as Russet does. With Russet closed, it starts the connection
itself; it lasts as long as the engine runs, which stops by itself after a while without work.

## Account changes and approvals

A command that changes your Apple developer account needs your approval:

- In a terminal, `russet provision` lists the changes and asks `Make them? [y/N]`.
- An agent's command cannot ask you. It stops with `perm.approvalRequired`, and the message names a plan and an
  approval. Approve it in Russet (it asks you in a dialog) or with `russet approve apr_…` in a terminal; the
  agent then finishes with `russet provision --plan pln_… --approval apr_…` within 10 minutes.

See [AI agents](ai-agents.md) for approvals in Russet, and `perm.approvalRequired` in the
[Error reference](errors.md).

## For AI agents

An agent that can run commands needs nothing more: tell it to use `russet` (`russet --help` lists what it can
do). An agent that speaks MCP gets the same through `russet mcp serve`: **Russet: Register Russet MCP with…**
registers it in your project (see [AI agents](ai-agents.md)).
