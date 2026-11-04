# Updates

Three parts of Russet update without a new setup:

- **Russet itself**: the editor, the Russet extension, its Windows programs and its engine in WSL, from
  Russet's GitHub releases.
- **The Swift toolchain** in the backend, from swift.org.
- **The virtual iPhone engine**, from vphone-linux on GitHub.

## How Russet updates itself

A Russet installed from its GitHub release checks for a newer release one minute after it starts, then every
six hours. It only considers published Windows releases, not drafts or pre-releases.

When there is one, a notification says which version is available, which one you have, and what it will
download:

- **a patch**, with its size and the parts it replaces, when only some parts changed;
- **the full installer**, when a patch does not fit your installation;
- **nothing to download**, when nothing Russet uses changed.

It offers three buttons:

- **Update and Restart** installs the update. Russet closes, updates and opens again.
- **Release Notes** opens the release's page on GitHub.
- **Skip This Version** stops Russet from offering this version by itself.
  **Russet: Check for Russet Updates** still shows it.

When Russet closes for an update, it closes as when you quit it yourself.

### A patch update

1. Russet downloads the changed parts into `%LOCALAPPDATA%\Russet\Updates\<version>` and checks each one
   against the SHA-256 the release publishes. If one does not match, nothing is changed.
2. Russet closes. The updater waits up to 10 minutes for every Russet window and process to close.
3. It replaces the changed parts. When the engine changed, it installs the new engine into WSL.
4. Russet opens again.

If Russet does not close, for example because a dialog about unsaved files is still open, the update is not
applied. The next time Russet starts, it says the version "was downloaded but not installed" and why. Select
**Try Again** to update again, or **Show Details** to see the updater's log, `apply.log`, in File Explorer.

### A full installer update

1. Russet downloads `RussetSetup-<version>.exe` into your temporary folder (`%TEMP%`) and checks it against
   the SHA-256 the release publishes. If it does not match, nothing is installed.
2. Russet closes and the installer runs without asking questions. It closes Russet's windows, ends the Russet
   programs that outlive the editor (such as the MCP server an agent keeps running), replaces Russet, and
   opens it again.

The installer's log is `%TEMP%\RussetSetup-<version>.log`.

### Check for updates yourself

- **Russet: Check for Russet Updates** checks now. It says when you have the latest release, and warns when
  GitHub could not be reached.
- **Russet: About Russet** shows the versions of Russet, its engine, the extension and the editor. Its
  **Check for Updates** button checks now, and **Copy** copies the versions.

A development installation says that it updates from source, and never offers releases.

### Russet's engine in WSL

Each time Russet starts, it compares its engine in WSL with the one the installed release carries. If they
differ, it stops the engine and copies the new one in. When that does not work, the **Russet backend** step on
the setup page says to update the engine; select **Set up** there. See [Getting started](getting-started.md).

### What happens to the virtual iPhone

- **A virtual iPhone being created** carries on. An update only closes Russet's Windows programs; the
  creation runs inside WSL. Open the setup page after the restart to see how far it is.
- **A running virtual iPhone** keeps running.

## Swift updates

While Russet is open, it checks every 30 seconds whether swift.org has a newer Swift release with a build for
the backend's Ubuntu. It reads swift.org's release list at most every six hours.

When there is one, Russet says it is updating Swift in the background, and installs it. The installation
needs about 8 GB free. When it is done, Russet says "Swift toolchain updated to … (from …)" and offers
**Roll Back**. Roll Back returns to the previous toolchain, and Russet does not install the release you rolled
back from again.

On the setup page, the Swift step shows the installed version, and the newer one when Swift is about to
update.

## Virtual iPhone engine updates

Russet builds new versions of the virtual iPhone engine beside the one in use and switches when no virtual
iPhone runs. See "How its engine updates" in [Virtual iPhone](virtual-iphone.md), which also explains what
"a Russet patch does not apply" means.

## Change how often Russet checks

The 30-second check for Swift and the virtual iPhone engine follows `russet.updates.checkIntervalSeconds`.
The Settings editor does not list it; add it to your user `settings.json`:

```json
{
  "russet.updates.checkIntervalSeconds": 300
}
```

The value is in seconds. `0` turns the checks off, and values below 10 count as 10. It does not change how
often Russet checks for its own releases.
