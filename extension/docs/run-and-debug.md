# Run and debug

Russet runs your app the way Xcode does: choose a run destination, then Run, Debug or Stop. Russet builds
the app, signs it, installs it and launches it.

| Shortcut | Command | When |
| --- | --- | --- |
| Ctrl+F5 | Russet: Run on iPhone | a project is open in a trusted folder |
| Shift+F5 | Russet: Stop | your app runs, outside a debug session |
| Ctrl+Alt+M | Russet: Mirror Screen | an iPhone is ready |

While a project is open, the status bar also has Run, Debug and Stop buttons.

## Choose where to run

The status bar shows the run destination, for example the iPhone's model and iOS version. Click it, or run
**Russet: Select Run Destination**. The picker has two groups:

- **iPhones**: iPhones connected to this PC, ready ones first. An iPhone that is not ready shows why, such as
  **Needs trust** or **Connecting**. With none connected, the picker says "No iPhone connected".
- **Virtual iPhones**: the virtual iPhone you created, with **Start** or **Stop** under it. If you have not
  created one, **Create a Virtual iPhone…** opens the setup page.

The **Devices** view in Russet's side bar lists the same devices. Right-click an iPhone and choose **Set as
Run Destination**.

The status bar also shows the target and build configuration, such as `MyApp · Debug`. Change them with
**Russet: Select Target** and **Russet: Select Configuration** (Debug or Release).

## Run your app on an iPhone

1. Connect and unlock the iPhone, and select it as the run destination.
2. Press Ctrl+F5, select the Run button in the status bar, or run **Russet: Run on iPhone**.
3. The **Running on iPhone** notification shows the step at work and how far the run is, such as
   "Compiling · 37%". Select **Cancel** on it to stop the run.
4. When the app starts, the Device Console opens and shows its output.

If the app's bundle identifier is a placeholder such as `com.example.myapp`, Russet first asks you to
replace it. See [Signing](signing.md).

## Run your app on the virtual iPhone

1. Start the virtual iPhone: in the run destination picker, select **Start** under it. See
   [Virtual iPhone](virtual-iphone.md).
2. Select the virtual iPhone as the run destination.
3. Run as above. The **Running on the virtual iPhone** notification shows "Building…", then
   "Installing" with your app's bundle identifier.
4. The Virtual iPhone tab opens with your app running.

The virtual iPhone signs the app itself, so it does not need your Apple account. If it is not running, Run
says so and asks you to start it first.

## Debug your app

1. Select the **Debug** configuration. Release builds cannot be debugged.
2. Select the Debug button in the status bar, or run **Russet: Debug on iPhone**.
3. Russet builds and installs the app, launches it paused, and attaches the debugger (LLDB).
4. Use the editor's Run and Debug view as usual: breakpoints, variables, stepping, the call stack and the
   Debug Console.

On the virtual iPhone, the notification says **Debugging on the virtual iPhone**. Russet launches the app
under the virtual iPhone's debug server and attaches to it.

The **Run Configurations** view in Russet's side bar lists the project's launch configurations, with Run and
Debug buttons next to each.

## Stop your app

Press Shift+F5, select Stop in the status bar, or run **Russet: Stop**. During a debug session, Stop ends the
session; the editor's own debug toolbar works too.

## Read the build log

Every build, run and debug writes to the **Russet Build** output. To open it, run **Russet: Show Build Log**,
or click the build item in the status bar.

Each job starts with a header, then one line per step with the time it took:

```
━━ Run MyApp · 10:42:01 ━━
▸ Finding the iPhone…
✔ Finding the iPhone (0.4 s)
▸ Checking the project…
✔ Checking the project (1.1 s)
...
▸ Compiling…
✔ Compiling (31.2 s)
...
✔ Run succeeded in 52.8 s (Finding the iPhone 0.4 s · Compiling 31.2 s · …)
```

The steps are: Finding the iPhone, Checking the project, Preparing, Compiling, Copying resources, Assembling
the app, Checking the app, Packaging, Signing, Installing on the iPhone, Launching and Attaching the
debugger. The compiler's own output appears between them.

When a job fails, its last lines say at which step it failed, list the compiler's errors with their file
and line, and suggest what to do. "Waiting for …" means the job waits for another one, such as a build of
the same project.

- Files on your Windows drives appear as Windows paths, such as
  `C:\Users\you\MyApp\Sources\ContentView.swift:10:13`. Ctrl+click one to open it at that line.
- A line longer than 600 characters is cut short. The full line is in the job's log file in your project's
  `.russet\logs` folder.
- By default the output leaves out the build tools' planning steps and the compiler's command lines. Set
  `russet.build.outputDetail` to `full` to see everything. The job's log file always has all of it.

When the output comes to the front depends on the setting `russet.build.revealOutput`:

- `onError` (the default): when a job fails.
- `always`: when a job starts, and when it fails.
- `never`: only when you open it.

## Watch progress in the status bar

While Russet builds or runs, the build item in the status bar shows the step and a percentage, such as
"Compiling 37%". The percentage only goes up. Afterwards it shows:

- a check mark and the time, after a successful build or run;
- an error icon and the number of errors, on a red background, after a failed one.

Click it to open the Russet Build output.

## Fix build errors in the Problems view

Compiler errors and warnings go to the Problems view (Ctrl+Shift+M), each with its file and line. Russet
clears them when the next build or run starts.

A failed run also shows a notification with the error, what to do and the error code. It has these buttons:

- **Show Build Log** opens the Russet Build output.
- **Open Docs** opens the code's section of the [Error reference](errors.md).

## Build without running

- **Russet: Build** runs the project's build task. Its terminal shows the same lines as the Russet Build
  output. Terminal › Run Build Task runs it too.
- **Russet: Clean Build Folder** deletes the project's build files and says how much space it freed.

Russet keeps a project's build files in its `.russet` folder.

## Read your app's output in the console

The **Device Console** panel shows your app's output and the iPhone's log entries for it. Run opens it and
shows the app's process only. Turn that off with the setting `russet.run.openConsole`; open it any time with
**Russet: Console**.

- **Clear** empties it.
- **Export…** saves its contents to a `.log` or `.txt` file.

The console keeps the last 50,000 lines. Change that with `russet.console.maxLines`.

## See the iPhone's screen

The **iPhone** view, in the secondary side bar's **iPhone & Agents** group, shows the run destination in a
device frame. Open it with **Russet: Mirror Screen**.

- For an iPhone on USB, it shows the live screen while the view is visible. Click or drag to touch, type to
  send keys, and use **Home**. The label under the phone says how the screen arrives:
  - **Live video**: the iPhone's screen as video.
  - **Screenshots · N/s, tap to control**: your PC's graphics cannot decode the iPhone's HEVC video in the
    editor, so the view shows screenshots, and a new one right after each touch. Touch, keys and **Home**
    still work.
  - **Screenshots · N/s**: the live stream could not start; the view tries it again later.
- For the virtual iPhone, select **Open in Editor**, or **Russet: Open Virtual iPhone**, to use its screen in
  the Virtual iPhone tab.

## Commands that are not available yet

These commands only say they are coming in a later update: **Russet: Install .ipa/.app…**,
**Russet: Take Screenshot**, **Russet: Crash Reports** and **Russet: Profiler**. To save the virtual iPhone's
screen, use **Take Screenshot** in the Virtual iPhone tab.
