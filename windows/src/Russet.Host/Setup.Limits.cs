using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using Microsoft.Win32;

// WSL limits the owner changes from the IDE ("Russet: Change WSL Limits"): WSL's memory limit,
// `memory=` under [wsl2] in %UserProfile%\.wslconfig (shared by every WSL distro, applied when WSL
// next starts), and Russet's disk limit, how far the backend's virtual disk may grow before
// updates stop building. Nothing here runs unless the owner asks for it in the IDE.
internal static partial class Setup
{
    static readonly string WslConfig = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".wslconfig");
    static readonly string RussetData = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Russet");
    static readonly string LimitsFile = Path.Combine(RussetData, "limits.json");
    // The owner's approved temporary storage (2026-11-01) until they choose another limit.
    const int DefaultDiskGB = 200;
    // An update builds a whole vphone tree beside the one in use (about 23 GB).
    const long UpdateHeadroomBytes = 25L << 30;
    const long GiB = 1L << 30;

    sealed record LimitsReport(int version, long machineMemoryBytes, int? memoryGB, long? wslMemoryBytes, bool restartNeeded,
        int diskGB, long? diskBytes, long? diskUsedBytes, long? freeBytes);

    static async Task<LimitsReport> Limits()
    {
        var machine = GC.GetGCMemoryInfo().TotalAvailableMemoryBytes;
        var memory = ConfiguredMemoryGB(File.Exists(WslConfig) ? File.ReadAllText(WslConfig) : "");
        long? effective = null;
        if ((await Capture(["--list", "--running", "--quiet"], Encoding.Unicode)).Output.Split('\n', StringSplitOptions.TrimEntries).Contains(Distribution)
            && MemTotal().Match((await Capture(["--distribution", Distribution, "--exec", "cat", "/proc/meminfo"])).Output) is { Success: true } total)
            effective = long.Parse(total.Groups[1].Value, CultureInfo.InvariantCulture) * 1024;
        // WSL's own default is half the machine's memory; the kernel keeps a few percent of either.
        var wanted = memory is { } gb ? gb * GiB : machine / 2;
        var restart = effective is { } e && Math.Abs(wanted - e) > wanted / 10;
        var disk = Vhdx();
        long? used = null;
        if (disk is not null && (await Capture(["--distribution", Distribution, "--exec", "df", "-B1", "--output=used", "/"])).Output.Split('\n') is [_, var line, ..]
            && long.TryParse(line.Trim(), CultureInfo.InvariantCulture, out var bytes))
            used = bytes;
        long? free = disk?.Directory?.Root is { } root ? new DriveInfo(root.FullName).AvailableFreeSpace : null;
        return new(1, machine, memory, effective, restart, DiskLimitGB(), disk?.Length, used, free);
    }

    static async Task<int> RunLimits(string id, string[] args)
    {
        void Emit(object value) => Console.WriteLine(JsonSerializer.Serialize(value));
        int Fail(string reason, string message)
        {
            Emit(new { step = id, @event = "failed", code = $"setup.{id}.{reason}", message, retryable = false });
            return 1;
        }
        var arguments = new Dictionary<string, int>();
        for (var i = 0; i < args.Length; i += 2)
        {
            var pair = i + 1 < args.Length && args[i] == "--arg" ? args[i + 1].Split('=', 2) : [];
            if (pair is not [("memoryGB" or "diskGB") and var key, var value] || !int.TryParse(value, CultureInfo.InvariantCulture, out var number)
                || !arguments.TryAdd(key, number))
                return Usage();
        }
        if (id == "wsl-restart") return arguments.Count == 0 ? RestartWsl(Emit) : Usage();
        var now = await Limits();
        var machineGB = (int)Math.Round(now.machineMemoryBytes / (double)GiB);
        if (arguments.TryGetValue("memoryGB", out var memoryGB))
        {
            if (memoryGB < 4 || memoryGB > machineGB)
                return Fail("memory", $"Choose a memory limit from 4 to {machineGB} GB (this PC has {machineGB} GB).");
            if (memoryGB != now.memoryGB)
            {
                Directory.CreateDirectory(RussetData);
                var before = File.Exists(WslConfig) ? File.ReadAllText(WslConfig) : "";
                // The owner's own file comes back from here if they want Russet's change undone.
                var backup = Path.Combine(RussetData, "wslconfig.before-russet");
                if (!File.Exists(backup)) File.WriteAllText(backup, before);
                File.WriteAllText(WslConfig, WithMemory(before, memoryGB));
                Emit(new { step = id, @event = "progress", message = $"WSL's memory limit is now {memoryGB} GB in %USERPROFILE%\\.wslconfig." });
            }
        }
        if (arguments.TryGetValue("diskGB", out var diskGB))
        {
            var floor = (int)Math.Ceiling((now.diskBytes ?? 0) / (double)GiB);
            if (diskGB < Math.Max(floor, 64) || diskGB > 4096)
                return Fail("disk", $"Choose a disk limit from {Math.Max(floor, 64)} GB (the backend's disk already takes {floor} GB) to 4096 GB.");
            Directory.CreateDirectory(RussetData);
            File.WriteAllText(LimitsFile, JsonSerializer.Serialize(new { version = 1, diskGB }));
            Emit(new { step = id, @event = "progress", message = $"Russet's disk limit is now {diskGB} GB." });
        }
        Emit(new { step = id, @event = "done" });
        return 0;
    }

    // An update that would grow the backend's disk past the owner's limit does not start. Space
    // freed inside the disk is reused before the file grows, so the file's size alone overstates it.
    static async Task<string?> DiskLimitBlocker()
    {
        var limits = await Limits();
        if (limits.diskBytes is not { } size) return null;
        var grown = Math.Max(size, (limits.diskUsedBytes ?? size) + UpdateHeadroomBytes);
        return grown <= limits.diskGB * GiB ? null
            : $"An update needs about {UpdateHeadroomBytes / GiB} GB more and would grow Russet's disk to {grown / GiB} GB, past its {limits.diskGB} GB limit. Raise the limit (Russet: Change WSL Limits) or free space, then retry.";
    }

    // Restarting WSL closes Russet first: a detached PowerShell waits for every process of this
    // installation to exit, shuts WSL down (all distros) so .wslconfig applies, and reopens Russet.
    static int RestartWsl(Action<object> emit)
    {
        var install = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, ".."));
        var launcher = Path.Combine(install, "Russet.Launcher.exe");
        if (!File.Exists(launcher))
        {
            emit(new { step = "wsl-restart", @event = "failed", code = "setup.wsl-restart.launcher", message = "Russet's launcher was not found; restart WSL by hand.", retryable = false });
            return 1;
        }
        static string Quote(string text) => "'" + text.Replace("'", "''") + "'";
        var script = $$"""
            $root = {{Quote(install)}}
            $deadline = (Get-Date).AddMinutes(2)
            while ((Get-Date) -lt $deadline -and (Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) })) { Start-Sleep -Milliseconds 500 }
            & {{Quote(Wsl)}} --shutdown
            Start-Process {{Quote(launcher)}}
            """;
        var start = new ProcessStartInfo(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "WindowsPowerShell", "v1.0", "powershell.exe"))
            { UseShellExecute = false, CreateNoWindow = true };
        foreach (var argument in new[] { "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-EncodedCommand", Convert.ToBase64String(Encoding.Unicode.GetBytes(script)) })
            start.ArgumentList.Add(argument);
        using var process = Process.Start(start);
        if (process is null)
        {
            emit(new { step = "wsl-restart", @event = "failed", code = "setup.wsl-restart.start", message = "WSL could not be restarted; restart it by hand.", retryable = true });
            return 1;
        }
        emit(new { step = "wsl-restart", @event = "done" });
        return 0;
    }

    static int DiskLimitGB()
    {
        try
        {
            using var limits = JsonDocument.Parse(File.ReadAllText(LimitsFile));
            return limits.RootElement.GetProperty("diskGB").GetInt32();
        }
        catch (Exception error) when (error is IOException or JsonException or KeyNotFoundException or InvalidOperationException or FormatException)
        {
            return DefaultDiskGB;
        }
    }

    // The backend's virtual disk, where WSL registered it.
    static FileInfo? Vhdx()
    {
        if (!OperatingSystem.IsWindows()) return null;
        using var lxss = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Lxss");
        foreach (var name in lxss?.GetSubKeyNames() ?? [])
        {
            using var entry = lxss!.OpenSubKey(name);
            if (entry?.GetValue("DistributionName") as string != Distribution || entry.GetValue("BasePath") is not string basePath) continue;
            var file = new FileInfo(Path.Combine(basePath.Replace(@"\\?\", ""), entry.GetValue("VhdFileName") as string ?? "ext4.vhdx"));
            return file.Exists ? file : null;
        }
        return null;
    }

    // `memory=` under [wsl2], in whole GB (WSL reads GB as GiB), or null when unset.
    internal static int? ConfiguredMemoryGB(string config)
    {
        var section = "";
        foreach (var raw in config.Split('\n'))
        {
            var line = raw.Trim();
            if (line.StartsWith('[')) section = line;
            else if (section.Equals("[wsl2]", StringComparison.OrdinalIgnoreCase) && MemorySetting().Match(line) is { Success: true } m)
            {
                var amount = double.Parse(m.Groups[1].Value, CultureInfo.InvariantCulture);
                var scale = m.Groups[2].Value.ToUpperInvariant() switch { "T" or "TB" => 1024.0, "M" or "MB" => 1 / 1024.0, "K" or "KB" => 1 / 1048576.0, "" or "B" => 1 / 1073741824.0, _ => 1.0 };
                return (int)Math.Round(amount * scale);
            }
        }
        return null;
    }

    // The owner's file with only `memory=` under [wsl2] set; everything else stays as written.
    internal static string WithMemory(string config, int gb)
    {
        var newline = config.Contains("\r\n", StringComparison.Ordinal) ? "\r\n" : "\n";
        var setting = $"memory={gb}GB";
        if (config.Trim().Length == 0) return $"[wsl2]{newline}{setting}{newline}";
        var lines = config.Replace("\r\n", "\n", StringComparison.Ordinal).Split('\n').ToList();
        var section = lines.FindIndex(l => l.Trim().Equals("[wsl2]", StringComparison.OrdinalIgnoreCase));
        if (section < 0)
        {
            if (lines[^1].Length > 0) lines.Add("");
            lines.InsertRange(lines.Count - 1, ["[wsl2]", setting]);
        }
        else
        {
            var end = lines.FindIndex(section + 1, l => l.TrimStart().StartsWith('['));
            if (end < 0) end = lines.Count;
            var key = lines.FindIndex(section + 1, end - section - 1, l => MemorySetting().IsMatch(l.Trim()));
            if (key >= 0) lines[key] = setting;
            else lines.Insert(section + 1, setting);
        }
        return string.Join(newline, lines);
    }

    [GeneratedRegex(@"^memory\s*=\s*([0-9]+(?:\.[0-9]+)?)\s*([A-Za-z]{0,2})\s*(?:[#;].*)?$", RegexOptions.IgnoreCase)]
    private static partial Regex MemorySetting();
    [GeneratedRegex(@"MemTotal:\s+(\d+) kB")]
    private static partial Regex MemTotal();
}
