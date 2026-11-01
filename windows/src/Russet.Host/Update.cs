using System.Diagnostics;
using System.IO.Compression;
using System.Text.Json;
using System.Text.Json.Nodes;

// Applies a patch update: only the parts of Russet that changed, which the IDE downloaded
// (extension/src/setup/selfUpdate.ts) into a staging folder as patch.json and one <part>.zip
// each. Run from a copy outside the installation, it waits for Russet to close (and changes
// nothing if Russet stays open), swaps each part, installs a new backend payload into WSL,
// records the new version and parts in installation.json, and starts Russet again.
//
//   Russet.Host.exe --apply-update --install <root> --staging <dir> --version <x.y.z>
internal static class Update
{
    // The parts a patch may replace, by their place in the installation.
    static readonly Dictionary<string, string> Parts = new(StringComparer.Ordinal)
    {
        ["extension"] = Path.Combine("editor", "resources", "app", "extensions", "russet"),
        ["host"] = "host",
        ["launcher"] = "Russet.Launcher.exe",
        ["backend"] = "backend",
    };

    internal static async Task<int> Apply(string[] args)
    {
        string? Option(string name)
        {
            var i = Array.IndexOf(args, name);
            return i >= 0 && i + 1 < args.Length ? args[i + 1] : null;
        }
        if (Option("--install") is not { } installArgument || Option("--staging") is not { } stagingArgument
            || Option("--version") is not { } version)
            return 2;
        var root = Path.GetFullPath(installArgument);
        var staging = Path.GetFullPath(stagingArgument);
        void Log(string message) =>
            File.AppendAllText(Path.Combine(staging, "apply.log"), $"{DateTime.Now:O} {message}{Environment.NewLine}");
        Dictionary<string, string> parts;
        try
        {
            using var patch = JsonDocument.Parse(File.ReadAllText(Path.Combine(staging, "patch.json")));
            parts = patch.RootElement.GetProperty("components").EnumerateObject()
                .ToDictionary(p => p.Name, p => p.Value.GetString() ?? "", StringComparer.Ordinal);
            if (parts.Count == 0 || parts.Keys.Any(name => !Parts.ContainsKey(name)))
                throw new InvalidDataException("patch.json names no part, or an unknown one");
        }
        catch (Exception error) when (error is IOException or JsonException or InvalidDataException or KeyNotFoundException or InvalidOperationException)
        {
            Log($"not applied: {error.Message}");
            return 1;
        }
        if (!await WaitForRussetToClose(root, TimeSpan.FromMinutes(10)))
        {
            Log("not applied: Russet stayed open");
            return 1;
        }
        var done = new List<string>();
        try
        {
            foreach (var name in parts.Keys)
            {
                var unpacked = Path.Combine(staging, name);
                if (Directory.Exists(unpacked)) Directory.Delete(unpacked, recursive: true);
                ZipFile.ExtractToDirectory(Path.Combine(staging, name + ".zip"), unpacked);
                Swap(Path.Combine(root, Parts[name]), unpacked);
                done.Add(name);
                Log($"replaced {name}");
            }
            if (parts.ContainsKey("backend"))
            {
                await Setup.StopDaemons();
                Log(await Setup.InstallPayload(Path.Combine(root, "backend"))
                    ? "installed the new engine into WSL"
                    : "the new engine did not install into WSL; the setup page's backend step repairs it");
            }
            var recordPath = Path.Combine(root, "installation.json");
            var record = JsonNode.Parse(File.ReadAllText(recordPath))?.AsObject() ?? [];
            record["version"] = version;
            var components = record["components"] as JsonObject ?? [];
            foreach (var (name, source) in parts) components[name] = source;
            record["components"] = components;
            File.WriteAllText(recordPath, record.ToJsonString(new JsonSerializerOptions { WriteIndented = true }));
            Log($"Russet {version}: {string.Join(", ", done)}");
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or InvalidDataException or JsonException)
        {
            Log($"stopped after [{string.Join(", ", done)}]: {error.Message}");
        }
        // Russet opens again either way: a part that was not replaced works as it did.
        Process.Start(new ProcessStartInfo(Path.Combine(root, "Russet.Launcher.exe")) { UseShellExecute = true })?.Dispose();
        return 0;
    }

    // A folder part moves in whole: the old one moves aside first and comes back if the move
    // fails. The launcher part is one file.
    static void Swap(string target, string unpacked)
    {
        if (!Directory.Exists(target) && Path.HasExtension(target))
        {
            File.Copy(Path.Combine(unpacked, Path.GetFileName(target)), target + ".new", overwrite: true);
            File.Move(target + ".new", target, overwrite: true);
            return;
        }
        var old = target + ".old";
        if (Directory.Exists(old)) Directory.Delete(old, recursive: true);
        if (Directory.Exists(target)) Directory.Move(target, old);
        try
        {
            Directory.Move(unpacked, target);
        }
        catch (IOException)
        {
            if (Directory.Exists(old) && !Directory.Exists(target)) Directory.Move(old, target);
            throw;
        }
        if (Directory.Exists(old)) Directory.Delete(old, recursive: true);
    }

    // Every process of this installation (not "Russet Development" beside it) has exited.
    static async Task<bool> WaitForRussetToClose(string root, TimeSpan limit)
    {
        var prefix = root.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        var deadline = DateTime.UtcNow + limit;
        while (DateTime.UtcNow < deadline)
        {
            var processes = Process.GetProcesses();
            var open = processes.Any(p => p.Id != Environment.ProcessId && PathOf(p) is { } path
                && path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase));
            foreach (var process in processes) process.Dispose();
            if (!open) return true;
            await Task.Delay(1000);
        }
        return false;
    }

    static string? PathOf(Process process)
    {
        try { return process.MainModule?.FileName; }
        catch (Exception error) when (error is System.ComponentModel.Win32Exception or InvalidOperationException or NotSupportedException) { return null; }
    }
}
