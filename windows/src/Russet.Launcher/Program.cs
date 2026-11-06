using System.Diagnostics;
using System.Runtime.InteropServices;

// Russet's launcher: a dedicated editor profile and backend, no changes to existing IDEs. A
// release installation (installation.json "type": "release") and a development one keep
// separate profiles and backend homes.
if (!OperatingSystem.IsWindows()) throw new PlatformNotSupportedException();
var caption = "Russet";
try
{
    var root = AppContext.BaseDirectory;
    var release = Installation.IsRelease(root);
    caption = release ? "Russet" : "Russet Development";
    var editor = Path.Combine(root, "editor", "Russet.exe");
    var host = Path.Combine(root, "host", "Russet.Host.exe");
    if (!File.Exists(editor) || !File.Exists(host)) throw new IOException($"The {caption} installation is incomplete. Run its installer again.");
    var (home, profile) = Installation.Backend(root);
    Directory.CreateDirectory(profile);
    var muxSocket = BackendDaemon.MuxSocket(home);
    var wsl = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "wsl.exe");
    // A first run has no backend yet: the editor opens anyway, and its setup page installs WSL
    // and the backend, then starts the daemon itself.
    var backendInstalled = Installation.BackendInstalled(wsl);
    if (backendInstalled && release) Installation.SyncBackend(wsl, root);
    // Physical iPhones through Apple Devices.
    System.Diagnostics.Process? bridge = null;
    var devices = backendInstalled && DeviceBridge.Start(wsl, host, muxSocket, out bridge);
    var start = BackendDaemon.Start(wsl, home, profile, devices);
    if (backendInstalled)
    using (var backend = Process.Start(start) ?? throw new IOException("Cannot start the Russet backend."))
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        var output = backend.StandardOutput.ReadToEndAsync(deadline.Token);
        var diagnostic = backend.StandardError.ReadToEndAsync(deadline.Token);
        try { await backend.WaitForExitAsync(deadline.Token); }
        catch (OperationCanceledException)
        {
            backend.Kill(entireProcessTree: true);
            throw new IOException("The Russet backend did not start in time. Check the managed runtime installation.");
        }
        // Do not copy raw daemon diagnostics, paths or protocol data into UI.
        await Task.WhenAll(output, diagnostic);
        if (backend.ExitCode != 0) throw new IOException("The Russet backend could not start. Check the managed runtime installation.");
    }
    var launch = new ProcessStartInfo(editor) { UseShellExecute = false };
    launch.Environment["RUSSET_WINDOWS_HOST"] = host;
    launch.Environment["RUSSET_BACKEND_HOME"] = home;
    launch.Environment["RUSSET_DEVICE_BACKEND"] = devices ? "live" : "none";
    launch.Environment["RUSSET_SECRETS_BACKEND"] = "passphrase";
    // Where this installation and its editor profile live (setup, self-update, the IDE MCP server).
    launch.Environment["RUSSET_INSTALL_ROOT"] = root;
    launch.Environment["RUSSET_IDE_PROFILE"] = profile;
    launch.Environment["RUSSET_IDE_DATA"] = profile;
    foreach (var value in new[] { "--user-data-dir", profile, "--extensions-dir", Path.Combine(profile, "extensions"), "--new-window", "--skip-release-notes" })
        launch.ArgumentList.Add(value);
    foreach (var value in args) launch.ArgumentList.Add(value);
    using var editorProcess = Process.Start(launch) ?? throw new IOException("Cannot open the Russet editor.");
    // The launcher has no window; it keeps the device bridge open until the editor exits.
    if (bridge is not null)
    {
        await editorProcess.WaitForExitAsync();
        DeviceBridge.Stop(bridge);
    }
}
catch (Exception error) when (error is IOException or System.ComponentModel.Win32Exception)
{
    Native.MessageBoxW(0, error.Message, caption, 0x10);
    Environment.ExitCode = 1;
}

internal static class Native
{
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    internal static extern int MessageBoxW(nint window, string text, string caption, uint type);
}
