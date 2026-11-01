using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

// Guided setup for the IDE's setup page: the "Setup runner contract" in
// docs/IDE-EXPERIENCE-PLAN.md. Steps call Russet's component scripts inside the
// managed distro; everything printed is redacted before it reaches the IDE.
internal static partial class Setup
{
    const string Distribution = "Russet-Dev";
    const string Lib = "/opt/russet/lib";
    const string AppleDevicesStore = "ms-windows-store://pdp/?productid=9NP83LWLPZ9K";
    static readonly string Wsl = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "wsl.exe");
    static readonly string[] Order = ["wsl", "wsl-update", "backend", "apple-devices", "iphone", "xtool", "xcode-sdk", "vphone", "virtual-iphone"];
    static readonly Dictionary<string, string> Titles = new()
    {
        ["wsl"] = "Windows Subsystem for Linux", ["wsl-update"] = "WSL pre-release", ["backend"] = "Russet backend", ["apple-devices"] = "Apple Devices (USB)",
        ["iphone"] = "iPhone", ["xtool"] = "xtool", ["xcode-sdk"] = "iOS SDK from Xcode",
        ["vphone"] = "Virtual iPhone engine", ["virtual-iphone"] = "Virtual iPhone",
    };

    internal static async Task<int> Run(string[] args)
    {
        if (args is ["status"])
        {
            Console.WriteLine(JsonSerializer.Serialize(new { version = 1, steps = await Status() }));
            return 0;
        }
        if (args is ["updates"])
        {
            // Polled by the IDE (every 30 s by default): one git ls-remote per component.
            var vphone = await Component("vphone_component.sh", "check");
            var components = vphone is { } v ? new[] { new { id = "vphone", title = Titles["vphone"], check = v } } : [];
            Console.WriteLine(JsonSerializer.Serialize(new { version = 1, components }));
            return vphone is null ? 1 : 0;
        }
        if (args is ["limits"])
        {
            Console.WriteLine(JsonSerializer.Serialize(await Limits()));
            return 0;
        }
        if (args is ["run", "wsl-limits" or "wsl-restart", ..])
            return await RunLimits(args[1], args[2..]);
        if (args.Length >= 2 && args[0] == "run" && Maintenance.TryGetValue(args[1], out var command))
        {
            var command2 = command.ToList();
            for (var i = 2; i < args.Length; i += 2)
            {
                // virtual-iphone-start takes which created VM to start; virtual-iphone-button which button.
                var pair = i + 1 < args.Length && args[i] == "--arg" ? args[i + 1].Split('=', 2) : [];
                if (args[1] == "virtual-iphone-button" && pair is ["name", "lock" or "volume-up" or "volume-down" or "home"])
                    command2.Add(pair[1]);
                // The built app: a Windows path (the IDE's view) or the backend's own.
                else if (args[1] is "virtual-iphone-install" or "virtual-iphone-debug" && pair is ["path", var path] && path.Length > 0
                    && (path.StartsWith('/') ? path : FramedPaths.ToLinux(Path.GetFullPath(path))) is { } linuxPath)
                    command2.Add(linuxPath);
                // Where the debug session's sockets go: the backend's runtime folder, which its relay opens.
                else if (args[1] == "virtual-iphone-debug" && pair is ["runtime", var runtime] && ManagedRuntime().IsMatch(runtime) && !runtime.Contains("/../"))
                    command2.AddRange(["--runtime", runtime]);
                else if (args[1] == "virtual-iphone-start" && pair is [("device" or "ios") and var key, var value] && Pairing().IsMatch(value))
                    command2.AddRange([$"--{key}", value]);
                else
                    return Usage();
            }
            return await RunMaintenance(args[1], [.. command2]);
        }
        if (args.Length >= 2 && args[0] == "run" && Order.Contains(args[1]))
        {
            var arguments = new Dictionary<string, string>();
            for (var i = 2; i < args.Length; i += 2)
            {
                var pair = i + 1 < args.Length && args[i] == "--arg" ? args[i + 1].Split('=', 2) : [];
                if (pair.Length != 2 || !arguments.TryAdd(pair[0], pair[1])) return Usage();
            }
            return await RunStep(args[1], arguments);
        }
        return Usage();
    }

    static int Usage()
    {
        Console.Error.WriteLine("usage: --setup status | --setup updates | --setup limits | --setup run <step> [--arg key=value]...");
        return 2;
    }

    // Runs outside the setup page's order, once their component is built: updating it from
    // upstream (built beside the build in use, then switched to, or back), and starting or
    // stopping the created virtual iPhone.
    static readonly Dictionary<string, string[]> Maintenance = new()
    {
        ["vphone-update"] = ["update"], ["vphone-retry-update"] = ["update", "--retry"], ["vphone-rollback"] = ["rollback"],
        ["virtual-iphone-start"] = ["start"], ["virtual-iphone-stop"] = ["stop"], ["virtual-iphone-unlock"] = ["unlock"],
        ["virtual-iphone-button"] = ["button"], ["virtual-iphone-install"] = ["install-app"],
        ["virtual-iphone-debug"] = ["debug-app"],
    };
    static readonly Dictionary<string, string> MaintenanceFailures = new()
    {
        ["vphone-update"] = "The virtual iPhone engine was not updated; the build in use is unchanged.",
        ["vphone-retry-update"] = "The virtual iPhone engine was not updated; the build in use is unchanged.",
        ["vphone-rollback"] = "The virtual iPhone engine was not rolled back.",
        ["virtual-iphone-start"] = "The virtual iPhone did not start.",
        ["virtual-iphone-stop"] = "The virtual iPhone did not stop.",
        ["virtual-iphone-unlock"] = "The virtual iPhone did not unlock.",
        ["virtual-iphone-button"] = "The virtual iPhone did not take the button.",
        ["virtual-iphone-install"] = "The app was not installed on the virtual iPhone.",
        ["virtual-iphone-debug"] = "The app could not be debugged on the virtual iPhone.",
    };

    static async Task<int> RunMaintenance(string id, string[] command)
    {
        void Emit(object value) => Console.WriteLine(JsonSerializer.Serialize(value));
        if (id is "vphone-update" or "vphone-retry-update" && await DiskLimitBlocker() is { } blocked)
        {
            Emit(new { step = id, @event = "failed", code = $"setup.{id}.disk-limit", message = blocked, retryable = true });
            return 1;
        }
        // A step's `RESULT {json}` line (virtual-iphone-debug: the debug session's sockets) rides on "done".
        JsonNode? result = null;
        if (await Stream(id, "vphone_component.sh", command, line => result = JsonNode.Parse(line)) != 0 || id == "virtual-iphone-debug" && result is null)
        {
            Emit(new { step = id, @event = "failed", code = $"setup.{id}.failed", message = MaintenanceFailures[id], retryable = true });
            return 1;
        }
        Emit(result is null ? new { step = id, @event = "done" } : new { step = id, @event = "done", result });
        return 0;
    }

    sealed record StepStatus(string id, string state, string title, string detail);

    static async Task<List<StepStatus>> Status()
    {
        var steps = new List<StepStatus>();
        void Add(string id, string state, string detail) => steps.Add(new(id, state, Titles[id], detail));
        var wsl = (await Capture(["--status"], Encoding.Unicode)).Exit == 0;
        Add("wsl", wsl ? "done" : "needed", wsl ? "Installed." : "Install WSL (administrator rights; may need a restart).");
        if (!wsl) Add("wsl-update", "unavailable", "Needs WSL.");
        else
        {
            var (wslVersion, direct3d) = await WslVersions();
            var updated = PrereleaseUpdated(wslVersion);
            Add("wsl-update", updated ? "done" : "needed", updated
                ? $"WSL {wslVersion} from the pre-release channel (Direct3D {direct3d ?? "unknown"})."
                : $"Russet needs WSL's pre-release channel (now WSL {wslVersion ?? "unknown"}, Direct3D {direct3d ?? "unknown"}) for GPU rendering of the virtual iPhone. Updating asks for administrator rights and stops WSL, the backend and the virtual iPhone for a minute.");
        }
        var backend = wsl && await BackendReady();
        Add("backend", backend ? "done" : wsl ? "needed" : "unavailable",
            backend ? "Ready." : BackendPayload() is not null
                ? "Install Russet's backend: Ubuntu in WSL (a 0.4 GB download) and Russet's engine."
                : "Russet's backend is missing; reinstall Russet to restore it.");
        var usb = await AppleDevicesReachable();
        Add("apple-devices", usb ? "done" : "needed", usb ? "Apple Devices is running." : "Install or open Apple Devices from the Microsoft Store.");
        Add("iphone", "unavailable", "Checked in the device picker once the backend is ready.");
        if (!backend)
        {
            foreach (var id in Order.SkipWhile(id => id != "xtool")) Add(id, "unavailable", "Needs the Russet backend.");
            return steps;
        }
        var xtool = await Component("xtool_component.sh");
        var hasXtool = xtool?.GetProperty("xtool").GetString() is { } version && version != "none";
        Add("xtool", hasXtool ? "done" : "needed", hasXtool ? "Installed." : "Not installed.");
        var sdk = xtool?.GetProperty("sdk").GetBoolean() == true;
        Add("xcode-sdk", sdk ? "done" : hasXtool ? "needed" : "unavailable",
            sdk ? "Installed." : "Choose an Xcode .xip to build the iOS SDK.");
        var vphone = await Component("vphone_component.sh");
        var built = vphone?.GetProperty("built").GetBoolean() == true;
        Add("vphone", built ? "done" : "needed", built ? "Built." : "Not built yet (about an hour).");
        var ready = vphone?.GetProperty("vmsReady").GetInt32() ?? 0;
        Add("virtual-iphone", ready > 0 ? "done" : built ? "needed" : "unavailable",
            ready > 0 ? $"{ready} ready." : "Create a virtual iPhone (several hours on first boot).");
        return steps;
    }

    static async Task<int> RunStep(string id, Dictionary<string, string> arguments)
    {
        void Emit(object value) => Console.WriteLine(JsonSerializer.Serialize(value));
        int Fail(string reason, string message, bool retryable = true)
        {
            Emit(new { step = id, @event = "failed", code = $"setup.{id}.{reason}", message, retryable });
            return 1;
        }
        var current = (await Status()).Single(s => s.id == id);
        if (current.state == "done") { Emit(new { step = id, @event = "done" }); return 0; }
        switch (id)
        {
            case "wsl":
                Emit(new { step = id, @event = "progress", message = "Waiting for the administrator prompt to install WSL." });
                try
                {
                    using var install = Process.Start(new ProcessStartInfo(Wsl, "--install --no-distribution") { UseShellExecute = true, Verb = "runas" });
                    if (install is null) return Fail("start", "WSL setup did not start.");
                    await install.WaitForExitAsync();
                    if (install.ExitCode != 0) return Fail("install", "WSL setup did not finish. Restart Windows if it asked to, then retry.");
                }
                catch (System.ComponentModel.Win32Exception) { return Fail("declined", "Administrator approval is needed to install WSL."); }
                break;
            case "wsl-update":
                Emit(new { step = id, @event = "progress", message = "Waiting for the administrator prompt; WSL, the backend and the virtual iPhone stop while WSL updates." });
                try
                {
                    using var update = Process.Start(new ProcessStartInfo(Wsl, "--update --pre-release") { UseShellExecute = true, Verb = "runas" });
                    if (update is null) return Fail("start", "The WSL update did not start.");
                    await update.WaitForExitAsync();
                    if (update.ExitCode != 0) return Fail("update", $"The WSL update did not finish (exit code {update.ExitCode}).");
                }
                catch (System.ComponentModel.Win32Exception) { return Fail("declined", "Administrator approval is needed to update WSL."); }
                var (after, _) = await WslVersions();
                if (after is null) return Fail("version", "WSL does not report its version after the update.");
                Directory.CreateDirectory(Path.GetDirectoryName(PrereleaseMarker)!);
                File.WriteAllText(PrereleaseMarker, JsonSerializer.Serialize(new { version = after, updated = DateTime.UtcNow }));
                break;
            case "backend":
                if (await InstallBackend(Emit, (reason, message, retryable) => Fail(reason, message, retryable)) != 0) return 1;
                break;
            case "apple-devices":
                Process.Start(new ProcessStartInfo(AppleDevicesStore) { UseShellExecute = true })?.Dispose();
                return Fail("missing", "Install Apple Devices from the Microsoft Store, open it, connect your iPhone, then retry.");
            case "iphone":
                return Fail("unavailable", "Connect and trust your iPhone, then pick it in the device picker.", retryable: false);
            case "xtool":
                if (await Stream(id, "xtool_component.sh", ["install"]) != 0) return Fail("install", "xtool did not install.");
                break;
            case "xcode-sdk":
                if (!arguments.TryGetValue("xip", out var xip) || !File.Exists(xip) || !xip.EndsWith(".xip", StringComparison.OrdinalIgnoreCase))
                    return Fail("xip", "Choose an Xcode .xip file.", retryable: false);
                if (FramedPaths.ToLinux(Path.GetFullPath(xip)) is not { } linuxXip)
                    return Fail("location", "Move the Xcode .xip into a folder Russet can read, then retry.", retryable: false);
                if (await Stream(id, "xtool_component.sh", ["sdk", linuxXip]) != 0) return Fail("build", "Building the iOS SDK failed.");
                break;
            case "vphone":
                if (await Stream(id, "vphone_component.sh", ["install"]) != 0) return Fail("build", "Building the virtual iPhone engine failed.");
                break;
            case "virtual-iphone":
                var device = arguments.GetValueOrDefault("device", "iPhone17,3");
                var ios = arguments.GetValueOrDefault("ios", "27.0.1");
                if (!Pairing().IsMatch(device) || !Pairing().IsMatch(ios)) return Fail("arguments", "Unknown device or iOS version.", retryable: false);
                if (await Stream(id, "vphone_component.sh", ["create", "--device", device, "--ios", ios]) != 0)
                    return Fail("create", "Creating the virtual iPhone stopped; retry resumes from the last step.");
                break;
        }
        Emit(new { step = id, @event = "done" });
        return 0;
    }

    // Russet ran `wsl --update --pre-release`, recording the version it reached: done while WSL is
    // at that version or newer (a later stable reinstall below it asks again).
    static readonly string PrereleaseMarker = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Russet", "wsl-prerelease.json");

    static bool PrereleaseUpdated(string? current)
    {
        try
        {
            using var marker = JsonDocument.Parse(File.ReadAllText(PrereleaseMarker));
            return Version.TryParse(marker.RootElement.GetProperty("version").GetString(), out var recorded)
                && Version.TryParse(current, out var now) && now >= recorded;
        }
        catch (Exception error) when (error is IOException or JsonException or KeyNotFoundException or InvalidOperationException) { return false; }
    }

    // `wsl --version` (UTF-16): the first line names WSL's version, another Direct3D's.
    static async Task<(string? Wsl, string? Direct3D)> WslVersions()
    {
        var lines = (await Capture(["--version"], Encoding.Unicode)).Output.Split('\n', StringSplitOptions.TrimEntries);
        string? Find(Func<string, bool> pick) => lines.Where(pick).Select(l => VersionNumber().Match(l)).FirstOrDefault(m => m.Success)?.Value;
        return (Find(_ => true), Find(l => l.Contains("Direct3D", StringComparison.OrdinalIgnoreCase)));
    }

    static async Task<bool> BackendReady()
    {
        var names = (await Capture(["--list", "--quiet"], Encoding.Unicode)).Output;
        return names.Split('\n', StringSplitOptions.TrimEntries).Contains(Distribution)
            && (await Capture(["--distribution", Distribution, "--user", "russet", "--exec", "test", "-x", "/opt/russet/bin/russet-dev"])).Exit == 0;
    }

    static async Task<bool> AppleDevicesReachable()
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(1));
        using var socket = new TcpClient(AddressFamily.InterNetwork);
        try { await socket.ConnectAsync(IPAddress.Loopback, 27015, deadline.Token); return true; }
        catch (Exception error) when (error is SocketException or OperationCanceledException) { return false; }
    }

    static async Task<JsonElement?> Component(string script, string command = "status")
    {
        var result = await Capture(["--distribution", Distribution, "--user", "russet", "--cd", "~", "--exec", "bash", $"{Lib}/{script}", command]);
        try { return result.Exit == 0 ? JsonDocument.Parse(result.Output).RootElement.Clone() : null; }
        catch (JsonException) { return null; }
    }

    static async Task<(int Exit, string Output)> Capture(string[] arguments, Encoding? encoding = null)
    {
        // wsl.exe's own messages are UTF-16; Linux programs' output is UTF-8.
        var start = new ProcessStartInfo(Wsl) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true,
            StandardOutputEncoding = encoding ?? Encoding.UTF8, StandardErrorEncoding = Encoding.UTF8 };
        foreach (var argument in arguments) start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new IOException("Cannot start WSL.");
        var output = process.StandardOutput.ReadToEndAsync();
        _ = process.StandardError.ReadToEndAsync();
        await process.WaitForExitAsync();
        return (process.ExitCode, (await output).Replace("\0", "").Trim());
    }

    // Runs a component step, turning its "==> " lines into progress and the rest into log events.
    static async Task<int> Stream(string id, string script, string[] arguments, Action<string>? result = null)
    {
        var start = new ProcessStartInfo(Wsl) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true,
            StandardOutputEncoding = Encoding.UTF8, StandardErrorEncoding = Encoding.UTF8 };
        // Start in the managed home, never the caller's Windows folder (unmounted once isolated).
        foreach (var argument in new[] { "--distribution", Distribution, "--user", "russet", "--cd", "~", "--exec", "bash", $"{Lib}/{script}" }.Concat(arguments))
            start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new IOException("Cannot start WSL.");
        var gate = new object();
        string? last = null;
        async Task Pump(StreamReader reader)
        {
            // ReadLine also splits on the carriage returns tools use to redraw progress.
            while (await reader.ReadLineAsync() is { } line)
            {
                if (result is not null && line.StartsWith("RESULT {", StringComparison.Ordinal))
                {
                    try { result(line[7..]); } catch (JsonException) { }
                    continue;
                }
                var text = Redact(line).Trim();
                if (text.Length == 0) continue;
                if (text.Length > 500) text = text[..500];
                string value;
                if (Phase().Match(text) is { Success: true } phase)
                    value = JsonSerializer.Serialize(new { step = id, @event = "progress", message = phase.Groups[1].Value,
                        fraction = int.Parse(phase.Groups[2].Value) / 100.0 });
                else if (text.StartsWith("==> ", StringComparison.Ordinal))
                    value = JsonSerializer.Serialize(new { step = id, @event = "progress", message = text[4..].Trim() });
                else
                    value = JsonSerializer.Serialize(new { step = id, @event = "log", message = text });
                lock (gate)
                {
                    if (value == last) continue;
                    last = value;
                    Console.WriteLine(value);
                }
            }
        }
        await Task.WhenAll(Pump(process.StandardOutput), Pump(process.StandardError));
        await process.WaitForExitAsync();
        return process.ExitCode;
    }

    // No private paths or device identifiers in what the IDE shows.
    static string Redact(string line)
    {
        // Upstream colours its "==>" status lines; the IDE renders plain text.
        line = Ansi().Replace(line, "");
        line = line.Replace("/home/russet", "~", StringComparison.Ordinal)
            .Replace(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "%USERPROFILE%", StringComparison.OrdinalIgnoreCase);
        return Identifier().Replace(UserMount().Replace(line, "%USERPROFILE%"), "<id>");
    }

    [GeneratedRegex(@"\d+\.\d+\.\d+(?:\.\d+)?")]
    private static partial Regex VersionNumber();
    [GeneratedRegex(@"^[A-Za-z0-9.,]{1,32}$")]
    private static partial Regex Pairing();
    [GeneratedRegex(@"^/home/russet/[A-Za-z0-9._/-]{1,200}$")]
    private static partial Regex ManagedRuntime();
    [GeneratedRegex(@"\x1B\[[0-9;?]*[A-Za-z]")]
    private static partial Regex Ansi();
    // xtool's "[Extracting XIP] 42%" redraws.
    [GeneratedRegex(@"^\[([A-Za-z][A-Za-z ]{0,40})\] (\d{1,3})%$")]
    private static partial Regex Phase();
    [GeneratedRegex(@"/mnt/[a-z]/Users/[^/\s]+", RegexOptions.IgnoreCase)]
    private static partial Regex UserMount();
    [GeneratedRegex(@"\b(?:0000[0-9A-Fa-f]{4}-[0-9A-Fa-f]{16}|[0-9a-fA-F]{40})\b")]
    private static partial Regex Identifier();
}
