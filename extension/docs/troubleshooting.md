# Troubleshooting

Start here when something does not work. Error notifications name an error code; the
[Error reference](errors.md) explains each one.

## Russet's engine is offline

The status bar shows **Russet: offline**, the Signing view says "Daemon not connected", and Run says that
the Russet daemon is not connected.

Russet reconnects by itself. When it cannot reach the engine, it starts it the way Russet does when it
opens, and keeps trying for about 30 seconds. The setting `russet.daemon.autoStart` turns that
start off; it is on by default.

If it stays offline:

1. Run **Russet: Restart Russet Daemon**. It asks the engine to stop, then starts it again.
   **Russet: Start Russet Daemon** only starts it.
2. Open the setup page (**Russet: Open Setup Guide**). If the **Russet backend** step is not done, set it up.
   While that step installs the engine, Russet leaves the engine alone; the step starts it when it is done.
3. Close Russet and open it again. If Russet cannot start the engine, it shows a message instead of opening,
   for example "The Russet backend did not start in time."
4. Look at the **Russet** output and the engine's log (see "Find the logs" below).

## A setup step fails

The step shows "This step did not finish" with the reason. Select **Retry**.

- Downloads carry on where they stopped.
- WSL reaches the internet on its own, apart from Windows. A work proxy, VPN or DNS can block WSL while
  Windows is online. Where WSL's own DNS answers nothing, Russet's backend uses public DNS servers.
- The backend needs 25 GB free on its drive, and Swift about 8 GB. To use another drive, see "Russet's WSL
  disk" below.

## WSL runs out of memory

Signs:

- A build fails with "the compiler was stopped before it finished (WSL ran out of memory)".
- The virtual iPhone stops with "WSL ran out of memory and stopped the virtual iPhone".
- The virtual iPhone step says WSL has less than the 8 GB it needs.

Unless you set a limit, WSL may use half of this PC's memory. To change it:

1. Run **Russet: Change WSL Limits (Memory and Disk)…**.
2. In **WSL memory limit (1 of 2)**, enter the GB WSL may use, from 4 up to this PC's memory. The virtual
   iPhone runs best with 16 GB.
3. In **Russet disk limit (2 of 2)**, keep the value unless you want to change it (see below).
4. Select **Set Limit**.
5. Select **Restart WSL and Russet** to apply it now. Russet stops the virtual iPhone, closes, shuts down
   every WSL distribution and opens again. Otherwise the limit applies the next time WSL starts, for example
   after Windows restarts.

Russet writes the limit as `memory=` under `[wsl2]` in `%USERPROFILE%\.wslconfig`, and leaves the rest of the
file as it was. Every WSL distribution shares that file. Before its first change, Russet keeps a copy of
your file as `%LOCALAPPDATA%\Russet\wslconfig.before-russet`.

You cannot restart WSL from Russet while a virtual iPhone is being created.

Other ways to free memory: stop the virtual iPhone while you build, and close other WSL distributions and
memory-hungry apps.

## Russet's WSL disk

Russet's backend lives in one virtual disk. Unless you chose another place, it is in
`%LOCALAPPDATA%\Russet\Backends\Russet-Dev\distro`.

### Change the disk limit

Russet's disk limit is how far updates of the virtual iPhone engine may grow that disk. It is 200 GB until
you change it. Set it in the second box of **Russet: Change WSL Limits (Memory and Disk)…**: from 64 GB, or
the disk's current size if larger, up to 4096 GB. Russet keeps it in `%LOCALAPPDATA%\Russet\limits.json`.

### Move the disk to another drive

1. Run **Russet: Move WSL Disk to Another Drive…**.
2. Choose a folder on a local drive. Russet puts the disk in a `Russet-Dev` folder inside it.
3. Confirm with **Move Disk**.
4. Russet stops the virtual iPhone and closes. A window shows the move, then Russet opens again.

Moving to another drive copies the whole disk, a few minutes per 10 GB. Only Russet's WSL distribution
stops. You cannot move the disk while a virtual iPhone is being created.

Before the backend is installed, the same command chooses where Russet will create it.

## Builds are slow

1. Open the **Russet Build** output (**Russet: Show Build Log**). Each step shows how long it took, and the
   last line of a job lists them all. Find the step that takes the time.
2. "Waiting for …" means the job waits for another one to finish, such as another build of the same project.
3. The first build of a project, and the first after **Russet: Clean Build Folder**, compiles everything,
   including the project's packages. Later builds reuse the build files in the project's `.russet` folder.
4. A running virtual iPhone uses six virtual processors and several GB of memory. Stop it while you build if
   you do not need it.
5. If WSL is short of memory, the compiler can be stopped part way. Raise WSL's memory limit as described
   above.

Updates of the virtual iPhone engine build in the background at the lowest processor priority, so they only
use processor time nothing else needs.

## Find the logs

| What | Where |
| --- | --- |
| The IDE: engine connection, updates, the doctor, the virtual iPhone's steps | Output panel › **Russet**, or **Russet: Show Daemon Log** |
| Builds, runs and debug sessions | Output panel › **Russet Build**, or **Russet: Show Build Log** |
| Every event of one build, run or debug job | your project's `.russet\logs\job_*.jsonl` |
| Russet's engine (one file per day) | `\\wsl.localhost\Russet-Dev\home\russet\.russet\ide\state\logs` |
| The creation of a virtual iPhone | `\\wsl.localhost\Russet-Dev\home\russet\vphone-setup-work\logs\create.log` |
| A virtual iPhone's virtual machine | `serial.log` in its folder under `\\wsl.localhost\Russet-Dev\home\russet\vphone-setup-work\devices` |
| A patch update | `%LOCALAPPDATA%\Russet\Updates\<version>\apply.log` |
| A full installer update | `%TEMP%\RussetSetup-<version>.log` |

Paste a `\\wsl.localhost\…` path into File Explorer's address bar to open it.

The setup page shows each step's own log under the step.

## Check this PC with the doctor

Run **Russet: Run Doctor**. Russet opens the **Russet** output and lists each check as passed, warned or
failed, with details. A notification says how many checks need attention.

## Report a bug

1. Run **Russet: About Russet** and select **Copy**. It copies the versions of Russet, its engine, the
   extension and the editor.
2. Open an issue at https://github.com/evilcomputer12/russet/issues and paste the versions.
3. Say what you did, what you expected, and what happened instead.
4. Add the error code and message from the notification, such as `build.compileFailed`.
5. Add the lines that matter from the logs above.

Before you send logs, remove your Apple ID, team ID, device names, device identifiers (UDIDs) and anything
else personal.

## Commands that are not available yet

These commands only say they are coming in a later update: **Install .ipa/.app…**, **Take Screenshot**,
**Sign Out**, **Fix Signing**, **Open Asset Catalog**, **New App Icon…**, **Publishing…**,
**Upload to TestFlight…**, **Profiler**, **Crash Reports**, **Agents**, **New Agent Session**,
**Ask Local Model…**, **Toggle MCP HTTP Endpoint** and **Install `russet` Shell Command**.

**Russet: Reveal Developer Mode** reaches the engine, which answers `core.notImplemented` in this release.
