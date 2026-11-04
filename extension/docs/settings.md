# Settings

Open the Settings editor with Ctrl+, and search for `russet`, or edit your user `settings.json`. This page
lists every Russet setting this release uses.

## Building and running

### russet.build.revealOutput

When the **Russet Build** output comes to the front.

- `onError` (default): when a build, run or debug fails.
- `always`: when one starts, and when one fails.
- `never`: only when you open it, with **Russet: Show Build Log** or the status bar.

See [Run and debug](run-and-debug.md).

### russet.build.outputDetail

How much of the build tools' output the **Russet Build** output shows.

- `concise` (default): each step, the compiler's errors and warnings, and a summary that lists the errors.
- `full`: everything the build tools print, including their planning steps and the compiler's command
  lines.

The job's log file in the project's `.russet\logs` folder always has all of it.

### russet.run.openConsole

Default: `true`. Opens the Device Console, showing only your app's output, after **Russet: Run on iPhone**
starts the app on an iPhone.

### russet.console.maxLines

Default: `50000`. How many lines the Device Console keeps; older lines are dropped. The console reads it when
its view loads.

## Signing

### russet.account.appleIdSignIn

Russet's own Apple ID sign-in. Default: `auto`.

- `auto`: sign-in is available. If this PC cannot create its own sign-in identity, the sign-in page says so
  and suggests an identity server.
- `on`: always try to sign in, even when Russet's engine does not report that this PC can create its own
  identity.
- `off`: sign-in is turned off. **Russet: Sign in with Apple ID…** points to this setting.

Russet never stores your password in settings.

### russet.account.anisetteServer

Default: empty. The identity (anisette) server used when you sign in with an Apple ID.

- Empty: this PC creates its own identity locally.
- An `https://` address: that server provides the identity instead. Its operator sees this computer's Apple
  device-identity requests, not your Apple ID password or two-factor codes.

Russet reads it from your user settings only; a workspace cannot change it. A changed value applies to the
next sign-in. See [Signing](signing.md).

### russet.project.organizationIdentifier

Default: empty. Your reverse-DNS prefix for new apps' bundle identifiers, such as `com.yourname`. Russet asks
for it the first time it needs it, and saves it here. `com.example` is refused, because Apple never
registers it.

## The engine connection

### russet.daemon.autoStart

Default: `true`. When the IDE cannot reach Russet's engine, it starts the engine in WSL itself. This also
lets **Russet: Start Russet Daemon** and **Russet: Restart Russet Daemon** start it. When it is off, only
opening Russet starts the engine. See [Troubleshooting](troubleshooting.md).

### russet.daemon.socketPath, russet.binDir and russet.debug.lldbDapPath

Defaults: empty. These point the IDE at a local engine socket, the `russet` command-line program and the
`lldb-dap` debugger. They have no effect in the Windows release, which reaches its engine and debugger
through the `Russet-Dev` WSL distribution.

## Settings the Settings editor does not show

Russet also reads these two settings. The Settings editor does not list them; add them to your user
`settings.json` by hand.

### russet.updates.checkIntervalSeconds

Default: `30`. How often, in seconds, Russet checks for updates of Swift and of the virtual iPhone engine.
`0` turns the checks off, and values below 10 count as 10. See [Updates](updates.md).

### russet.mirror.liveStream

Default: `true`. Whether the **iPhone** view asks a USB iPhone for its live screen stream. Set it to `false`
to show screenshots only.

## Other Russet settings

The Settings editor lists more `russet.*` settings, such as `russet.defaultDevice`,
`russet.build.configuration`, `russet.mirror.maxFps` and the `russet.agents.*`, `russet.ai.*` and
`russet.mcp.*` settings. This release does not use them yet; changing them has no effect.
