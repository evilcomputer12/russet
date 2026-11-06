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
        if (await WaitForRussetToClose(root, TimeSpan.FromMinutes(10), Log) is { } open)
        {
            // The IDE reads this line on its next start and offers the update again (selfUpdate.ts).
            Log($"not applied: Russet stayed open ({open})");
            // Reopened unless its window is still there (quitting was cancelled).
            var prefix = root.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
            var left = Open(prefix);
            if (!left.Any(HasWindow)) Reopen(root);
            foreach (var process in left) process.Dispose();
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
                if (name == "host") RefreshCli(root);
                done.Add(name);
                Log($"replaced {name}");
            }
            if (parts.ContainsKey("backend"))
            {
                using var maintenance = new Mutex(false, BackendDaemon.MaintenanceMutex);
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
        Reopen(root);
        return 0;
    }

    // bin\russet.exe, the command line (Cli.cs), is the host's program under another name: it follows a new host.
    static void RefreshCli(string root)
    {
        var cli = Path.Combine(root, "bin", "russet.exe");
        Directory.CreateDirectory(Path.GetDirectoryName(cli)!);
        File.Copy(Path.Combine(root, "host", "Russet.Host.exe"), cli + ".new", overwrite: true);
        File.Move(cli + ".new", cli, overwrite: true);
    }

    static void Reopen(string root) =>
        Process.Start(new ProcessStartInfo(Path.Combine(root, "Russet.Launcher.exe")) { UseShellExecute = true })?.Dispose();

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

    // Until every process of this installation (not "Russet Development" beside it) has exited: null, or what
    // stayed open. The editor's windows close first (the IDE quit itself; a dialog about unsaved files may hold it
    // up to `limit`), and the launcher then closes the device bridge. Some processes outlive the editor: the Russet
    // IDE MCP server an agent keeps running (editor\Russet.exe as Node), a socket relay or a setup step whose IDE is
    // gone. Those serve no editor any more and hold the files being replaced, so after a grace period they are ended
    // (the process only: a setup step's WSL process and a VM it started keep running).
    static async Task<string?> WaitForRussetToClose(string root, TimeSpan limit, Action<string> log)
    {
        var prefix = root.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        var deadline = DateTime.UtcNow + limit;
        DateTime? windowsClosed = null;
        while (DateTime.UtcNow < deadline)
        {
            var open = Open(prefix);
            try
            {
                if (open.Count == 0) return null;
                if (open.Any(p => HasWindow(p))) windowsClosed = null;
                else windowsClosed ??= DateTime.UtcNow;
                if (windowsClosed is { } since && DateTime.UtcNow - since > TimeSpan.FromSeconds(20))
                {
                    foreach (var process in open)
                    {
                        log($"ending {process.ProcessName} ({process.Id}), which outlived the editor");
                        try { process.Kill(entireProcessTree: false); }
                        catch (Exception error) when (error is System.ComponentModel.Win32Exception or InvalidOperationException) { }
                    }
                    windowsClosed = DateTime.UtcNow;
                }
            }
            finally
            {
                foreach (var process in open) process.Dispose();
            }
            await Task.Delay(1000);
        }
        var left = Open(prefix);
        var names = string.Join(", ", left.Select(p => p.ProcessName).Distinct());
        foreach (var process in left) process.Dispose();
        return names.Length > 0 ? names : null;
    }

    static List<Process> Open(string prefix)
    {
        var open = new List<Process>();
        foreach (var process in Process.GetProcesses())
        {
            if (process.Id != Environment.ProcessId && PathOf(process) is { } path
                && path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
                open.Add(process);
            else
                process.Dispose();
        }
        return open;
    }

    static bool HasWindow(Process process)
    {
        try { return process.MainWindowHandle != 0; }
        catch (InvalidOperationException) { return false; }
    }

    static string? PathOf(Process process)
    {
        try { return process.MainModule?.FileName; }
        catch (Exception error) when (error is System.ComponentModel.Win32Exception or InvalidOperationException or NotSupportedException) { return null; }
    }
}
