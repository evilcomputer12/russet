using System.Diagnostics;
using System.Text.Json;

// What the launcher learns about its installation before starting anything.
internal static class Installation
{
    // installation.json beside the launcher: "type" is "release" (the GitHub installer) or
    // "development" (a developer's update-dev-install.ps1 copy).
    internal static bool IsRelease(string root)
    {
        try
        {
            using var record = JsonDocument.Parse(File.ReadAllText(Path.Combine(root, "installation.json")));
            return record.RootElement.TryGetProperty("type", out var type) && type.GetString() == "release";
        }
        catch (Exception error) when (error is IOException or JsonException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    // This installation's backend home and editor profile: a release and a development installation keep their
    // own. The launcher and `russet` on the command line (Russet.Host's Cli.cs) use the same.
    internal static (string Home, string Profile) Backend(string root)
    {
        var release = IsRelease(root);
        var profile = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Russet",
            release ? "IDE" : "IDE-Development");
        return (release ? "/home/russet/.russet/ide" : "/home/russet/.russet-dev/ide-development", profile);
    }

    // The managed distribution with its engine: WSL answers, Russet-Dev exists and holds the
    // daemon. False on a first run, before the setup page has installed them.
    internal static bool BackendInstalled(string wsl) =>
        File.Exists(wsl) && Run(wsl, ["-d", "Russet-Dev", "-u", "russet", "--exec", "test", "-x", "/opt/russet/bin/russet-dev"], 60, out _) == 0;

    // The installation's engine and backend scripts, copied into the distribution again when an
    // update replaced them (the full installer changes only Windows' files), with the daemons
    // stopped first so the start that follows runs the new engine. One that cannot be copied is
    // left to the setup page's backend step, which reports it.
    internal static void SyncBackend(string wsl, string root)
    {
        if (BackendSync.Payload(root) is not { } payload || BackendSync.LinuxPath(payload) is not { } linux) return;
        var id = BackendSync.Id(payload);
        if (Run(wsl, BackendSync.Recorded("Russet-Dev"), 60, out var recorded) == 0 && recorded.Trim() == id) return;
        // An editor already open (a second launch) must not start the old engine meanwhile.
        using var maintenance = new Mutex(false, BackendDaemon.MaintenanceMutex);
        Run(wsl, BackendSync.StopDaemons("Russet-Dev"), 60, out _);
        // A changed /etc/wsl.conf applies once the distribution restarts; the daemon starts it again.
        if (Run(wsl, BackendSync.Install("Russet-Dev", linux, id), 300, out _) == BackendSync.RestartExit)
            Run(wsl, BackendSync.Restart("Russet-Dev"), 60, out _);
    }

    // Runs wsl.exe: its exit status within `seconds` (-1 where it did not start or finish), and its standard output.
    static int Run(string wsl, string[] arguments, int seconds, out string output)
    {
        output = "";
        var start = new ProcessStartInfo(wsl)
        {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true,
            // Its own (empty) input: wsl.exe would otherwise read the caller's (`russet` on the command line).
            RedirectStandardInput = true,
        };
        foreach (var argument in arguments) start.ArgumentList.Add(argument);
        try
        {
            using var process = Process.Start(start);
            if (process is null) return -1;
            process.StandardInput.Close();
            var standard = process.StandardOutput.ReadToEndAsync();
            var errors = process.StandardError.ReadToEndAsync();
            if (!process.WaitForExit(seconds * 1000))
            {
                process.Kill(entireProcessTree: true);
                return -1;
            }
            Task.WaitAll(standard, errors);
            output = standard.Result;
            return process.ExitCode;
        }
        catch (System.ComponentModel.Win32Exception)
        {
            return -1;
        }
    }
}
