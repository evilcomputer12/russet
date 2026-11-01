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

    // The managed distribution with its engine: WSL answers, Russet-Dev exists and holds the
    // daemon. False on a first run, before the setup page has installed them.
    internal static bool BackendInstalled(string wsl)
    {
        if (!File.Exists(wsl)) return false;
        var start = new ProcessStartInfo(wsl)
        {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true,
        };
        foreach (var argument in new[] { "-d", "Russet-Dev", "-u", "russet", "--exec", "test", "-x", "/opt/russet/bin/russet-dev" })
            start.ArgumentList.Add(argument);
        try
        {
            using var probe = Process.Start(start);
            if (probe is null) return false;
            var output = probe.StandardOutput.ReadToEndAsync();
            var errors = probe.StandardError.ReadToEndAsync();
            if (!probe.WaitForExit(60_000))
            {
                probe.Kill(entireProcessTree: true);
                return false;
            }
            Task.WaitAll(output, errors);
            return probe.ExitCode == 0;
        }
        catch (System.ComponentModel.Win32Exception)
        {
            return false;
        }
    }
}
