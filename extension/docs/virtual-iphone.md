# Virtual iPhone

The virtual iPhone is an iPhone that runs inside your PC. iOS runs in a virtual machine (QEMU) in Russet's
WSL distribution. The engine behind it is vphone-linux, an open-source project
(https://github.com/YahiaAngelo/vphone-linux), which Russet builds with a few fixes of its own.

You run and debug apps on it like on a USB iPhone, and use its screen in the **Virtual iPhone** editor tab.
It does not need your Apple account: the virtual iPhone signs the apps you install on it.

## What it needs

- The **WSL pre-release** and **Virtual iPhone engine** steps of the setup page. See
  [Getting started](getting-started.md).
- At least 8 GB of memory for WSL. It runs best with 16 GB. With less, the virtual iPhone gets less memory of
  its own (between 4 and 8 GB) and runs more slowly. Change WSL's memory with
  **Russet: Change WSL Limits (Memory and Disk)…**.

In WSL no GPU driver reaches the virtual iPhone, so your processor draws its screen.

## Create a virtual iPhone

1. Run **Russet: Open Setup Guide** and go to the **Virtual iPhone** step.
2. Choose the **Device** and the **iOS version**. The lists show which pairings are tested, and the default:
   iPhone 16 Pro with iOS 27.0.1.
3. Select **Set up**.

Creating it takes several hours. The step shows where it is, out of nine steps:

1. checking WSL
2. preparing the VM folder
3. downloading iOS
4. setting up the GPU driver
5. preparing the firmware
6. patching the firmware
7. restoring iOS (with a percentage)
8. installing the custom firmware
9. starting iOS for the first time

While a step has no percentage, the page shows how many minutes it has taken so far.

### While it is being created

- The creation carries on when you close Russet, when Russet updates itself, and when you select
  **Cancel**. Open the setup page again to see how far it is; the step shows done when it finishes.
- Only one virtual iPhone can be created at a time.
- Russet does not stop the virtual machine, restart WSL or move Russet's WSL disk while a creation runs. It
  says that a virtual iPhone is still being created.
- If the creation stops, select **Retry**. It resumes from the last step that finished. If a restore of iOS
  was interrupted, Russet resets the virtual disk and restores again.

The creation writes its log to
`\\wsl.localhost\Russet-Dev\home\russet\vphone-setup-work\logs\create.log`. The log of the previous creation
is kept beside it as `create.log.1`.

## Start the virtual iPhone

1. Open the run destination picker (click the run destination in the status bar), and select **Start** under
   the virtual iPhone. You can also select **Start** under it in the **Devices** view, or
   **Start Virtual iPhone** in the Virtual iPhone tab.
2. Wait. It takes several minutes to reach the lock screen; Russet waits up to 20 minutes.
3. Russet unlocks it, lets it settle for about two minutes, and restarts its home screen so the wallpaper and
   widgets appear.
4. The Virtual iPhone tab opens with its screen.

Russet starts the device and iOS version selected in the setup page's Virtual iPhone step.

The virtual iPhone keeps running in WSL when you close Russet. Stop it when you no longer need it: it uses
six virtual processors and several GB of memory.

## Stop the virtual iPhone

Select **Stop** under it in the run destination picker or in the **Devices** view. Russet asks iOS to shut
down, and ends the virtual machine if it does not stop within about two minutes.

## Use its screen

Open the tab with **Russet: Open Virtual iPhone**, or by selecting the virtual iPhone in the **Devices** view.
It shows the screen while the tab is visible.

- Click or drag on the screen to touch it.
- Type while the screen has focus. Press Escape to leave the screen.
- Swipe up from the bottom edge to unlock and go to the home screen.

The tab's toolbar has these buttons. The shortcuts work while the tab is active:

| Button | Shortcut | What it does |
| --- | --- | --- |
| Home | Ctrl+Shift+H | Unlocks the iPhone and goes to the home screen. |
| Lock | Ctrl+Shift+L | Presses the side button. |
| Volume Up | Ctrl+Shift+Up | Presses volume up. |
| Volume Down | Ctrl+Shift+Down | Presses volume down. |
| Rotate Left | Ctrl+Shift+Left | Turns the device frame in the tab. |
| Rotate Right | Ctrl+Shift+Right | Turns the device frame in the tab. |
| Take Screenshot | Ctrl+Shift+S | Saves the screen as a PNG file you choose. |

Rotating turns the frame only; iOS itself does not rotate yet. While the tab is open, the same actions are in
the Command Palette under **Russet Virtual iPhone**.

## Run and debug apps on it

Select the virtual iPhone as the run destination, then use Run or Debug as usual. See
[Run and debug](run-and-debug.md).

## How its engine updates

Russet keeps the virtual iPhone engine on vphone-linux's newest version. While Russet is open, it checks
GitHub every 30 seconds.

1. When there is a new version, Russet says it is "updating … in the background" and builds it beside the
   build in use. The build takes an hour or more, at the lowest processor priority. The build in use keeps
   working meanwhile.
2. Russet switches to the new build once no virtual iPhone runs and none is being created. If one runs, it
   tells you that the update is built and waits for the virtual iPhone to stop.
3. After the switch it says "updated to … (from …)" and offers **Roll Back**.

**Roll Back** goes back to the previous build. Stop the virtual iPhone first. The version you rolled back from
is not used again until you retry it. Russet keeps the build in use and the one before it, and removes older
builds.

An update needs room: about 25 GB more in Russet's WSL disk, within Russet's disk limit, and 30 GB free on
`C:`. When the disk limit would be passed, Russet does not start the update and offers
**Change WSL Limits** and **Retry**.

### When an update fails

Russet tells you once per new version, with the reason, and offers **Retry**. The build in use stays as it
was, and the virtual iPhone keeps working.

### When a Russet patch does not apply

Russet applies a few fixes of its own (patches) to vphone-linux. When vphone-linux changes the code one of
them touches, the patch no longer applies, and Russet cannot build that version. Russet then says that
upstream changed code Russet patches, so it cannot be built yet.

- Nothing breaks: the build in use stays in use and keeps working.
- Wait for a Russet update. Russet updates bring patches that fit the new version. Select
  **Check for Russet Updates** on the notification, or see [Updates](updates.md).
- **Retry** fails the same way until such a Russet update is installed.

## If something goes wrong

- "WSL ran out of memory and stopped the virtual iPhone": raise WSL's memory limit with
  **Russet: Change WSL Limits (Memory and Disk)…**, close other WSL distributions and memory-hungry apps,
  then try again. See [Troubleshooting](troubleshooting.md).
- "the virtual iPhone stopped while starting": its virtual machine log is `serial.log` in its folder, such as
  `\\wsl.localhost\Russet-Dev\home\russet\vphone-setup-work\devices\vphone-iPhone17-3-2701\` for an
  iPhone17,3 with iOS 27.0.1.
- Starting, stopping, installing on and debugging the virtual iPhone write their progress to the **Russet**
  output. Run **Russet: Show Daemon Log** to open it.
