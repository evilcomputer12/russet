# Getting started

This page takes you from the installer to a PC that can build iOS apps. Russet's engine runs in its own WSL
distribution, `Russet-Dev`. The setup page installs it and everything else Russet needs, one step at a time.

## Install Russet

You need 64-bit Windows 10 version 2004 (build 19041) or later, or Windows 11.

1. Download `RussetSetup-<version>.exe` from Russet's releases page:
   https://github.com/evilcomputer12/russet/releases
2. Run it. It installs for your Windows user only and does not ask for administrator rights.
3. Choose whether you want a desktop shortcut.
4. Leave **Open Russet** ticked and finish.

Russet installs into `%LOCALAPPDATA%\Programs\Russet` and adds **Russet** to the Start menu. Its editor keeps
its own profile in `%LOCALAPPDATA%\Russet\IDE`, so your other editors and their settings are not touched.

Uninstalling Russet (Windows Settings › Apps) leaves the `Russet-Dev` WSL distribution and your projects in
place.

## Start Russet for the first time

Open **Russet** from the Start menu. On every start, Russet:

1. updates its engine inside WSL if the installed release carries a newer one;
2. connects to Apple Devices, so it can reach an iPhone on USB;
3. starts the engine;
4. opens the editor.

On a new PC there is no engine yet. The editor opens anyway, and the **Set up Russet** page opens by itself.
While the WSL, WSL pre-release or Russet backend step still needs doing, Russet opens the setup page in each
new window and shows a warning with **Open Setup**.

To open the page later, run **Russet: Open Setup Guide** from the Command Palette (Ctrl+Shift+P).
**Russet: Open Getting Started Walkthrough** opens a shorter checklist of the same steps.

## Use the setup page

The page lists every step in order. Each one shows its state, a short description, a progress bar while it
runs, and the last lines of its log.

- **Set up remaining steps** runs every step that still needs work, in order. It stops at the first step that
  fails. It skips the Xcode step until you have chosen an Xcode archive, and Apple Devices, which you install
  from the Microsoft Store yourself (only a real iPhone needs it).
- **Stop** cancels the step that is running.
- **Check again** reads the state of every step again.
- Each step also has its own button: **Set up**, **Retry**, **Repair**, **Cancel** or **Check again**.

Only one step runs at a time. Steps that download carry on where they stopped when you retry them.

**Repair** on a finished step runs it again, over what is installed, after saying what it will do. Your
projects, your sign-in and your virtual iPhones stay. For example, Repair on the Russet backend installs its
engine again and restarts it, and Repair on Swift installs swift.org's newest release again (about 1.1 GB).
Apple Devices, the iPhone and a created virtual iPhone have no Repair.

Setup is complete once one way to run apps is ready: a real iPhone (Apple Devices and the iPhone step), or the
virtual iPhone (its engine and a created virtual iPhone). You need not set up both.

| Step | Needs | Administrator | Download | Time |
| --- | --- | --- | --- | --- |
| Windows Subsystem for Linux | nothing | yes | from Microsoft | may need a restart |
| WSL pre-release | WSL | yes | from Microsoft | about a minute |
| Russet backend | WSL, 25 GB free | no | Ubuntu, 0.4 GB | |
| Swift toolchain | backend, about 8 GB free | no | about 1.1 GB | |
| Apple Devices (USB) | Microsoft Store | no | from the Store | |
| iPhone | Apple Devices, an iPhone, a cable | no | | |
| xtool | backend | no | xtool 1.24.0 | |
| Sign in to your Apple developer account | engine running | no | about 2 MB | |
| iOS SDK from Xcode | xtool, an Xcode `.xip` | no | none (you download Xcode) | |
| Virtual iPhone engine | backend | no | its source code and build tools | about an hour |
| Virtual iPhone | engine, WSL with 8 GB of memory or more | no | iOS for the chosen device | several hours |

## Windows Subsystem for Linux

Russet's engine runs in WSL 2.

1. Select **Set up**. Windows asks for administrator approval.
2. Approve it. Russet runs `wsl --install --no-distribution`.
3. Restart Windows if WSL setup asks you to, then select **Check again**.

You can also run `wsl --install --no-distribution` yourself in an administrator terminal. The page has a
**Copy command** button for it.

## WSL pre-release

Russet requires WSL from its pre-release channel for the virtual iPhone.

1. Select **Set up** and approve the administrator prompt.
2. Russet runs `wsl --update --pre-release`. WSL, Russet's engine and a running virtual iPhone stop for about
   a minute while WSL updates.

The step stays done while WSL is at the version Russet updated it to, or newer. If WSL later goes back to an
older stable version, the step asks again.

## Russet backend

This step creates the `Russet-Dev` distribution and installs Russet's engine into it.

1. Make sure the drive that will hold the backend has 25 GB free. By default it is the drive of
   `%LOCALAPPDATA%`. To use another drive, run **Russet: Move WSL Disk to Another Drive…** first and choose a
   folder there.
2. Select **Set up**. Russet downloads Ubuntu for WSL (0.4 GB), checks its checksum and creates the
   distribution.
3. Russet installs its engine and starts it. The IDE connects to it when the step is done.

When a Russet update brings a new engine, Russet copies it into WSL when it starts. If that did not work, this
step shows "Update Russet's engine in WSL to this release's". Select **Set up**; it takes under a minute.

## Swift toolchain

Russet installs the newest Swift release from swift.org that has a build for the backend's Ubuntu.

1. Select **Set up**. The download is about 1.1 GB, and the drive needs about 8 GB free.
2. Russet installs the Linux libraries Swift needs, checks the download's signature against swift.org's keys,
   and unpacks it.

WSL reaches the internet on its own, apart from Windows. If the step says Ubuntu could not install the
libraries Swift needs, check whether a work proxy, VPN or DNS blocks WSL, then select **Retry**. The Swift
download is kept for the retry.

Swift then updates itself when swift.org publishes a newer release. See [Updates](updates.md).

## Apple Devices (USB)

Windows talks to an iPhone over USB through Apple's **Apple Devices** app.

1. Select **Set up**, or **Get Apple Devices from the Microsoft Store**. The Store page opens.
2. Install Apple Devices and open it.
3. Connect your iPhone, then select **Retry**.

The step is done while Apple Devices is running. Only a real iPhone needs it: **Set up remaining steps** leaves
it to its own button, and you can skip it if you only use the virtual iPhone.

Russet finds your iPhone as soon as Apple Devices runs, also when you open Apple Devices after Russet. You do
not need to restart Russet.

## iPhone

This step is done in the run destination picker, not on the page.

1. Connect the iPhone with a USB cable and unlock it.
2. When the iPhone asks whether to trust this computer, tap **Trust**.
3. Turn on Developer Mode: on the iPhone, open Settings › Privacy & Security › Developer Mode, turn it on and
   restart the iPhone.
4. Select **Choose iPhone…** and pick your iPhone.

The step is done when Russet reports the iPhone as ready. If the picker shows **Needs trust**, unlock the
iPhone and tap **Trust**, or run **Russet: Pair…**.

## xtool

Russet uses xtool to turn an Xcode archive into an iOS SDK. Select **Set up**. Russet downloads the pinned
xtool release into the backend and checks its checksum.

## Sign in to your Apple developer account

This step comes from the engine, so it is available once the engine runs. Select **Sign in with Apple ID…**
or **Add App Store Connect API key…**. See [Signing](signing.md).

## iOS SDK from Xcode

Russet reads the iOS SDK from the Xcode archive you download from Apple with your Apple ID. It never
redistributes Xcode.

1. Select **Download Xcode from Apple** and download an `Xcode_<version>.xip` to your Downloads folder.
2. Back on the page, the step lists every `Xcode_*.xip` in Downloads, newest first. Pick one, or select
   **Browse…** to choose an archive somewhere else.
3. Select **Set up**. Russet extracts the archive and builds the SDK in the backend.

The archive must be on a drive with a letter, such as `C:\`, not on a network path. If the step asks you to
move the archive, move it into a folder on such a drive, then retry.

**Russet: Import Xcode (.xip or Xcode.app)…** opens this step.

## Virtual iPhone engine

This step builds vphone-linux, the open-source engine that runs iOS in a virtual machine.

1. Select **Set up**.
2. Russet prepares the backend's system settings, downloads the engine's source with Russet's patches, and
   builds it. This takes about an hour.

See [Virtual iPhone](virtual-iphone.md) for how this engine updates.

## Virtual iPhone

1. Under **Device**, choose a model. The default is iPhone 16 Pro (iPhone17,3).
2. Under **iOS version**, choose a version. The default is iOS 27.0.1. Versions marked **untested** have not
   been verified end to end yet; creating them may fail.
3. Select **Set up**.

Creating a virtual iPhone takes several hours. It goes through nine steps, and the page shows where it is,
such as "Step 7 of 9: restoring iOS", with a percentage while iOS is restored.

The creation carries on while Russet is closed, and **Cancel** does not stop it either. When you open Russet
again, the step shows how far it is, and it shows done when it finishes. Only one virtual iPhone can be
created at a time. If the creation stops, select **Retry**: it resumes from the last step that finished.

WSL needs 8 GB of memory or more for this step. If it has less, the step says so; raise the limit with
**Russet: Change WSL Limits (Memory and Disk)…** (see [Troubleshooting](troubleshooting.md)).

## Create your first app

1. Run **Russet: New Project…**.
2. Enter the app's name, and choose the folder to create it in.
3. The first time, Russet asks for your organization identifier, such as `com.yourname`. It saves it in the
   setting `russet.project.organizationIdentifier`.
4. Confirm or change the bundle identifier it suggests.
5. Choose **Open** or **Open in New Window**.

The new app is a Swift package with a `russet.toml` file. Next, see [Run and debug](run-and-debug.md).
