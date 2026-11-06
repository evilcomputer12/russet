using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

// `russet` on Windows: bin\russet.exe, this program under another name, on the user's PATH. It is the engine's
// own command line (build, run, debug, devices, screenshots, logs, signing, approvals, `mcp serve` …), run in
// Russet's WSL distribution as the launcher runs the daemon (the same home, sign-in and iPhones), with Windows
// paths in and out; plus what only Windows has: the virtual iPhone (`russet vm …`) and the setup steps
// (`russet setup …`). Nothing needs Russet's window, so terminals, scripts and AI agents use it as they would
// xcodebuild or flutter. `Russet.Host.exe --cli …` is the same.
internal static class Cli
{
    static readonly string Wsl = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "wsl.exe");

    // The installation: bin\ or host\ beside installation.json.
    static string Root => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, ".."));

    // The host program for this installation's own modes (--setup, --device-bridge): host\Russet.Host.exe, so
    // that a long-running one does not hold bin\russet.exe, which updates replace.
    static string HostProgram => Path.Combine(Root, "host", "Russet.Host.exe") is { } host && File.Exists(host) ? host : Environment.ProcessPath!;

    // Run by a program, not in a terminal (an agent, a script, an MCP client): child processes get no console
    // window of their own.
    static bool Headless => Console.IsInputRedirected && Console.IsOutputRedirected && Console.IsErrorRedirected;

    const string Help = """
        russet: Russet's command line on Windows. It runs Russet's engine in WSL, without Russet's window.

        Windows only:
          russet vm status                     the virtual iPhone: built, created, running, being created
          russet vm create [--device D] [--ios V]
                                               create one (hours on first boot; carries on in the background)
          russet vm start [--device D --ios V] | stop | unlock
          russet vm button lock|home|volume-up|volume-down
          russet vm install APP                install and launch an .app folder or .ipa on it
          russet vm debug APP [LLDB ARGS…]     install, launch stopped, and debug it with LLDB here
                                               (e.g. -b -o "breakpoint set -n main" -o continue -o bt)
          russet setup status                  the setup steps and their state
          russet setup run STEP [--repair] [--xip PATH]
                                               run (or repair) one, e.g. backend, swift, xtool, xcode-sdk, vphone

        Everything else is the engine's command line, e.g.:
          russet doctor · russet device list · russet build · russet run [--device UDID] · russet debug
          russet logs --app BUNDLE_ID --follow · russet screen screenshot --output shot.png
          russet provision · russet approve apr_…
          russet mcp serve --client claude-code   (MCP over stdio, for AI agents)
        Add --json for machine-readable output. Windows paths work in arguments and come back in output.

        """;

    internal static async Task<int> Run(string[] args)
    {
        var (home, profile) = Installation.Backend(Root);
        // What this installation's own modes read (Setup.cs), as the launcher sets it for the editor.
        Environment.SetEnvironmentVariable("RUSSET_BACKEND_HOME", home);
        Environment.SetEnvironmentVariable("RUSSET_IDE_PROFILE", profile);
        Environment.SetEnvironmentVariable("RUSSET_INSTALL_ROOT", Root);
        switch (args)
        {
            case [] or ["help" or "--help" or "-h"]:
                Console.Out.Write(Help);
                return args.Length == 0 ? 2 : await Engine(["--help"], home, profile);
            case ["vm", .. var rest]:
                return await VirtualIphone(rest, home);
            case ["setup", .. var rest]:
                return await SetupCommand(rest);
            default:
                return await Engine(args, home, profile);
        }
    }

    // ── The engine ──────────────────────────────────────────────────────────────────────────────────────────

    static async Task<int> Engine(string[] args, string home, string profile)
    {
        if (!Installation.BackendInstalled(Wsl))
            return Fail("Russet's backend is not installed yet: open Russet and finish its setup page, or run `russet setup run backend`.");
        // Help and the version need no daemon, so no iPhones either.
        var devices = args.Any(a => a is "--help" or "-h" or "--version" or "-V") || await EnsureBridge(home);
        var cwd = Directory.GetCurrentDirectory();
        var start = BackendDaemon.Cli(Wsl, home, profile, devices, FramedPaths.ToLinux(cwd) is null ? null : cwd, args.Select(ToLinux));
        var json = args.Contains("--json");
        if (args is ["mcp", "serve", ..])
            return await Mcp(start);
        // The result on standard output gets Windows paths. Standard input stays this console's, and so does
        // standard error where it is one, so the engine asks its questions (approvals, sign-in) in the terminal.
        start.CreateNoWindow = Headless;
        start.RedirectStandardOutput = true;
        start.StandardOutputEncoding = Encoding.UTF8;
        var errors = Console.IsErrorRedirected;
        if (errors)
        {
            start.RedirectStandardError = true;
            start.StandardErrorEncoding = Encoding.UTF8;
        }
        using var process = Process.Start(start) ?? throw new IOException("Cannot start WSL.");
        // Ctrl+C reaches the engine through WSL; this process waits for it to end.
        Console.CancelKeyPress += (_, e) => e.Cancel = true;
        var output = Pump(process.StandardOutput, Console.Out, json);
        var diagnostics = errors ? Pump(process.StandardError, Console.Error, json) : Task.CompletedTask;
        await Task.WhenAll(output, diagnostics);
        await process.WaitForExitAsync();
        return process.ExitCode;
    }

    // MCP over stdio: one JSON message per line, its path fields mapped both ways (FramedPaths).
    static async Task<int> Mcp(ProcessStartInfo start)
    {
        start.CreateNoWindow = true;
        start.RedirectStandardInput = true;
        start.RedirectStandardOutput = true;
        using var process = Process.Start(start) ?? throw new IOException("Cannot start WSL.");
        using var cancel = new CancellationTokenSource();
        var inbound = Task.Run(async () =>
        {
            try { await FramedPaths.CopyLines(Console.OpenStandardInput(), process.StandardInput.BaseStream, toLinux: true, cancel.Token); }
            catch (IOException) { }
            finally { process.StandardInput.Close(); }
        });
        await FramedPaths.CopyLines(process.StandardOutput.BaseStream, Console.OpenStandardOutput(), toLinux: false, cancel.Token);
        await process.WaitForExitAsync();
        cancel.Cancel();
        return process.ExitCode;
    }

    // A Windows path in an argument, alone or as `--option=C:\…`, as the backend sees it.
    internal static string ToLinux(string argument)
    {
        var equals = argument.StartsWith("--", StringComparison.Ordinal) ? argument.IndexOf('=') : -1;
        var (prefix, value) = equals > 0 ? (argument[..(equals + 1)], argument[(equals + 1)..]) : ("", argument);
        return FramedPaths.ToLinux(value) is { } linux ? prefix + linux : argument;
    }

    // The backend's /mnt/x/… paths in a line of output as Windows paths: with forward slashes in JSON, which then
    // needs no escaping.
    internal static string ToWindows(string line, bool json) => FramedPaths.WindowsPaths(line, forwardSlashes: json);

    static async Task Pump(StreamReader reader, TextWriter writer, bool json)
    {
        var buffer = new char[8192];
        var pending = new StringBuilder();
        int count;
        while ((count = await reader.ReadAsync(buffer)) > 0)
        {
            pending.Append(buffer, 0, count);
            var text = pending.ToString();
            var end = text.LastIndexOf('\n');
            if (end < 0) continue;
            await writer.WriteAsync(ToWindows(text[..(end + 1)], json));
            await writer.FlushAsync();
            pending.Clear().Append(text[(end + 1)..]);
        }
        if (pending.Length > 0) await writer.WriteAsync(ToWindows(pending.ToString(), json));
        await writer.FlushAsync();
    }

    // iPhones for a daemon this command may start: the bridge of Russet's window (the launcher's or the IDE's), or
    // one started here, which lasts as long as the daemon (`--device-bridge --until-idle`). Started through the
    // shell, it inherits none of this process's handles: a caller reading this command's output does not wait
    // for it. False when no bridge comes up; the command then runs without iPhones.
    static async Task<bool> EnsureBridge(string home)
    {
        var socket = BackendDaemon.MuxSocket(home);
        if (DeviceBridge.Served(Wsl, socket)) return true;
        Process.Start(new ProcessStartInfo(HostProgram, "--device-bridge --until-idle")
        {
            UseShellExecute = true, WindowStyle = ProcessWindowStyle.Hidden,
        })?.Dispose();
        for (var i = 0; i < 20; i++)
        {
            await Task.Delay(1000);
            if (DeviceBridge.Served(Wsl, socket)) return true;
        }
        return false;
    }

    // ── The virtual iPhone and the setup steps, through this installation's setup runner (Setup.cs) ──────────

    static async Task<int> VirtualIphone(string[] args, string home)
    {
        var json = args.Contains("--json");
        args = [.. args.Where(a => a != "--json")];
        switch (args)
        {
            case ["status"]:
                var steps = await Status(json, ["vphone", "virtual-iphone"]);
                // Whether one runs, which the setup steps do not say.
                if (steps == 0 && !json && await Component() is { } vm && vm.TryGetProperty("running", out var running))
                    Console.WriteLine($"{"running",-16} {(running.GetBoolean() ? "yes" : "no")}");
                return steps;
            case ["create", .. var options]:
                return Options(options, ["--device", "--ios"]) is { } create
                    ? await Step(["run", "virtual-iphone", .. create], json) : Usage();
            case ["start", .. var options]:
                return Options(options, ["--device", "--ios"]) is { } pairing
                    ? await Step(["run", "virtual-iphone-start", .. pairing], json) : Usage();
            case [("stop" or "unlock") and var command]:
                return await Step(["run", $"virtual-iphone-{command}"], json);
            case ["button", ("lock" or "home" or "volume-up" or "volume-down") and var button]:
                return await Step(["run", "virtual-iphone-button", "--arg", $"name={button}"], json);
            case ["install", var app]:
                return await Step(["run", "virtual-iphone-install", "--arg", $"path={Path.GetFullPath(app)}"], json);
            case ["debug", var app, .. var lldb]:
                return await Debug(Path.GetFullPath(app), lldb, home);
            default:
                return Usage();
        }
    }

    static async Task<int> SetupCommand(string[] args)
    {
        var json = args.Contains("--json");
        args = [.. args.Where(a => a != "--json")];
        switch (args)
        {
            case ["status"] or []:
                return await Status(json, null);
            case ["run", var step, .. var options]:
                var repair = options.Contains("--repair");
                if (Options([.. options.Where(o => o != "--repair")], ["--xip", "--device", "--ios"]) is not { } arguments) return Usage();
                // The Xcode archive as the runner checks it: a full Windows path.
                for (var i = 0; i + 1 < arguments.Length; i += 2)
                    if (arguments[i + 1].StartsWith("xip=", StringComparison.Ordinal))
                        arguments[i + 1] = "xip=" + Path.GetFullPath(arguments[i + 1][4..]);
                return await Step(["run", step, .. arguments, .. (repair ? new[] { "--arg", "repair=1" } : [])], json);
            default:
                return Usage();
        }
    }

    // `--device D --ios V` as the runner's `--arg device=D --arg ios=V`; null for anything else.
    static string[]? Options(string[] options, string[] allowed)
    {
        var arguments = new List<string>();
        for (var i = 0; i < options.Length; i += 2)
        {
            if (i + 1 >= options.Length || !allowed.Contains(options[i])) return null;
            arguments.AddRange(["--arg", $"{options[i][2..]}={options[i + 1]}"]);
        }
        return [.. arguments];
    }

    static async Task<int> Status(bool json, string[]? only)
    {
        var (exit, output) = await Runner(["status"]);
        if (exit != 0) return Fail("The setup runner did not answer.");
        using var status = JsonDocument.Parse(output);
        var steps = status.RootElement.GetProperty("steps").EnumerateArray()
            .Where(s => only is null || only.Contains(s.GetProperty("id").GetString())).ToList();
        if (json)
        {
            Console.WriteLine(JsonSerializer.Serialize(steps));
            return 0;
        }
        foreach (var step in steps)
        {
            var progress = step.TryGetProperty("progress", out var p) && p.GetString() is { } where ? $" ({where})" : "";
            Console.WriteLine($"{step.GetProperty("id").GetString(),-16} {step.GetProperty("state").GetString(),-12} {step.GetProperty("detail").GetString()}{progress}");
        }
        return 0;
    }

    // Runs one setup or virtual iPhone step, showing its progress; its result (if any) on standard output.
    static async Task<int> Step(string[] arguments, bool json, Action<JsonNode>? result = null)
    {
        var start = new ProcessStartInfo(HostProgram) { UseShellExecute = false, CreateNoWindow = Headless, RedirectStandardInput = true, RedirectStandardOutput = true, StandardOutputEncoding = Encoding.UTF8 };
        foreach (var argument in new[] { "--setup" }.Concat(arguments)) start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new IOException("Cannot start Russet's setup runner.");
        process.StandardInput.Close();
        Console.CancelKeyPress += (_, e) => e.Cancel = true;
        var failed = false;
        while (await process.StandardOutput.ReadLineAsync() is { } line)
        {
            if (json) Console.WriteLine(line);
            JsonNode? e;
            try { e = JsonNode.Parse(line); }
            catch (JsonException) { continue; }
            var message = e?["message"]?.GetValue<string>();
            switch (e?["event"]?.GetValue<string>())
            {
                case "progress" when !json:
                    var fraction = e["fraction"]?.GetValue<double>();
                    Console.Error.WriteLine($"==> {message}{(fraction is { } f ? $" ({f:P0})" : "")}");
                    break;
                case "log" when !json:
                    Console.Error.WriteLine(message);
                    break;
                case "failed":
                    failed = true;
                    if (!json) Console.Error.WriteLine($"error: {message} ({e["code"]?.GetValue<string>()})");
                    break;
                case "done" when e["result"] is { } value:
                    if (result is not null) result(value);
                    else if (!json) Console.WriteLine(value.ToJsonString());
                    break;
            }
        }
        await process.WaitForExitAsync();
        return failed || process.ExitCode != 0 ? 1 : 0;
    }

    // vphone_component.sh status, or null.
    static async Task<JsonElement?> Component()
    {
        var start = new ProcessStartInfo(Wsl) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = true, RedirectStandardOutput = true };
        foreach (var argument in new[] { "-d", "Russet-Dev", "-u", "russet", "--cd", "~", "--exec", "bash", "/opt/russet/lib/vphone_component.sh", "status" })
            start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new IOException("Cannot start WSL.");
        process.StandardInput.Close();
        var output = await process.StandardOutput.ReadToEndAsync();
        await process.WaitForExitAsync();
        try { return process.ExitCode == 0 ? JsonDocument.Parse(output).RootElement.Clone() : null; }
        catch (JsonException) { return null; }
    }

    static async Task<(int Exit, string Output)> Runner(string[] arguments)
    {
        var start = new ProcessStartInfo(HostProgram) { UseShellExecute = false, CreateNoWindow = Headless, RedirectStandardInput = true, RedirectStandardOutput = true, StandardOutputEncoding = Encoding.UTF8 };
        foreach (var argument in new[] { "--setup" }.Concat(arguments)) start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new IOException("Cannot start Russet's setup runner.");
        process.StandardInput.Close();
        var output = await process.StandardOutput.ReadToEndAsync();
        await process.WaitForExitAsync();
        return (process.ExitCode, output);
    }

    // `russet vm debug`: the IDE's Debug on the virtual iPhone (install, launch stopped, debugserver attached),
    // then LLDB here in the terminal on that session (vphone_debug.py lldb), with LLDB's own arguments after APP.
    static async Task<int> Debug(string app, string[] lldb, string home)
    {
        JsonNode? session = null;
        var exit = await Step(["run", "virtual-iphone-debug", "--arg", $"path={app}", "--arg", $"runtime={home}/runtime"], json: false,
            value => session = value);
        if (exit != 0 || session is null) return exit == 0 ? Fail("The debug session did not start.") : exit;
        var start = new ProcessStartInfo(Wsl) { UseShellExecute = false };
        foreach (var argument in new[] { "-d", "Russet-Dev", "-u", "russet", "--exec", "python3", "/opt/russet/lib/vphone_debug.py", "lldb",
            session.ToJsonString() }.Concat(lldb.Select(ToLinux))) start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new IOException("Cannot start WSL.");
        Console.CancelKeyPress += (_, e) => e.Cancel = true;
        await process.WaitForExitAsync();
        return process.ExitCode;
    }

    static int Usage()
    {
        Console.Error.Write(Help);
        return 2;
    }

    static int Fail(string message)
    {
        Console.Error.WriteLine($"error: {message}");
        return 1;
    }
}
