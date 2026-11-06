using System.Diagnostics;
using System.IO.Pipes;
using System.Text.Json;
using System.Runtime.Versioning;

// Separate Windows supervisor experiment. The Rust daemon remains the engine.
if (!OperatingSystem.IsWindows()) throw new PlatformNotSupportedException("Windows host required.");
if (args.SequenceEqual(new[] { "--device-mux", "stdio" }))
{
    await DeviceMux.Run();
    return;
}
// The Apple Devices bridge for a window whose launcher started none, because Russet opened before its backend
// was installed: "ready" once iPhones are reachable, then the IDE holds this process, and with it the bridge,
// until the window closes (EOF on standard input). Another launcher's bridge already serving is "ready" too.
// With --until-idle (`russet` on the command line, with no window open), it lasts as long as the daemon runs.
if (args is ["--device-bridge", .. var bridgeOptions] && bridgeOptions is [] or ["--until-idle"])
{
    var home = Environment.GetEnvironmentVariable("RUSSET_BACKEND_HOME");
    if (home is null || !home.StartsWith("/home/russet/", StringComparison.Ordinal) || home.Contains("/../", StringComparison.Ordinal))
        throw new ArgumentException("RUSSET_BACKEND_HOME must be the managed backend's home.");
    var wsl = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "wsl.exe");
    if (!DeviceBridge.Start(wsl, Environment.ProcessPath!, BackendDaemon.MuxSocket(home), out var owned))
    {
        Console.WriteLine("unavailable");
        Environment.ExitCode = 1;
        return;
    }
    Console.WriteLine("ready");
    if (owned is not null)
    {
        if (bridgeOptions is ["--until-idle"]) await DeviceBridge.UntilIdle(wsl, BackendDaemon.DaemonSocket(home));
        else await Console.In.ReadToEndAsync();
        DeviceBridge.Stop(owned);
    }
    return;
}
// `russet` on the command line (bin\russet.exe is this program), after this program's own modes above and below.
if (args is not ["--setup", ..] && (args is ["--cli", ..]
    || string.Equals(Path.GetFileNameWithoutExtension(Environment.ProcessPath), "russet", StringComparison.OrdinalIgnoreCase)))
{
    Environment.ExitCode = await Cli.Run(args is ["--cli", .. var cli] ? cli : args);
    return;
}
// A patch update, run from a copy outside the installation (Update.cs).
if (args is ["--apply-update", ..])
{
    Environment.ExitCode = await Update.Apply(args);
    return;
}
if (args is ["--setup", .. var setupArguments])
{
    Environment.ExitCode = await Setup.Run(setupArguments);
    return;
}
var options = new Dictionary<string, string>();
for (var i = 0; i < args.Length; i += 2)
{
    if (i + 1 >= args.Length || !args[i].StartsWith("--", StringComparison.Ordinal))
        throw new ArgumentException("Expected --distribution NAME --socket PATH --helper PATH [--pipe NAME].");
    if (!options.TryAdd(args[i], args[i + 1])) throw new ArgumentException("Duplicate option.");
}
string Required(string key) => options.TryGetValue(key, out var value) && value.Length > 0
    ? value : throw new ArgumentException($"Missing {key}.");
var distribution = Required("--distribution");
if (distribution != "Russet-Dev") throw new ArgumentException("Only the managed Russet-Dev distribution is supported.");
var socketPath = Required("--socket");
var helperPath = Required("--helper");
var protocol = options.GetValueOrDefault("--protocol", "raw");
if (protocol is not ("raw" or "rpc" or "dap")) throw new ArgumentException("Invalid relay protocol.");
if (!socketPath.StartsWith("/home/russet/", StringComparison.Ordinal) || socketPath.Contains("/../", StringComparison.Ordinal))
    throw new ArgumentException("Socket must be in the managed user's home.");
if (helperPath != "/opt/russet/lib/socket_relay.py") throw new ArgumentException("Unrecognized backend helper.");
var pipeName = options.GetValueOrDefault("--pipe", $"russet-dev-{Guid.NewGuid():N}");
if (!pipeName.StartsWith("russet-dev-", StringComparison.Ordinal) || pipeName.Any(c => !char.IsAsciiLetterOrDigit(c) && c != '-'))
    throw new ArgumentException("Invalid host pipe name.");

using var stop = new CancellationTokenSource();
Console.CancelKeyPress += (_, e) => { e.Cancel = true; stop.Cancel(); };
Process StartHelper(bool probe)
{
    var start = new ProcessStartInfo(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "wsl.exe"))
    {
        UseShellExecute = false, CreateNoWindow = true,
        RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true,
    };
    foreach (var argument in new[] { "--distribution", distribution, "--user", "russet", "--exec", "python3", "-I", helperPath, "--socket", socketPath })
        start.ArgumentList.Add(argument);
    if (probe) start.ArgumentList.Add("--probe");
    return Process.Start(start) ?? throw new IOException("Cannot start backend relay.");
}

// Readiness is a validated backend connection, not process existence.
using (var probe = StartHelper(true))
{
    try
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        var stderr = probe.StandardError.ReadToEndAsync(deadline.Token);
        var ready = await probe.StandardOutput.ReadToEndAsync(deadline.Token);
        await probe.WaitForExitAsync(deadline.Token);
        if (probe.ExitCode != 0 || ready.Trim() != "ready")
            throw new IOException($"Backend is not ready: {(await stderr).Trim()}");
    }
    finally
    {
        if (!probe.HasExited) { probe.Kill(entireProcessTree: true); await probe.WaitForExitAsync(); }
    }
}

var clients = new List<Task>();
try
{
    // Random session names avoid a fixed endpoint. The protected user/SYSTEM
    // DACL and PIPE_REJECT_REMOTE_CLIENTS enforce a local account boundary.
    using var first = CreatePipe();
    Console.WriteLine(JsonSerializer.Serialize(new { state = "ready", endpoint = @"\\.\pipe\" + pipeName, distribution, securityVerified = true }));
    await first.WaitForConnectionAsync(stop.Token);
    clients.Add(Relay(first));
    // The first pipe must remain alive until its relay finishes.
    await clients[0];
}
catch (OperationCanceledException) when (stop.IsCancellationRequested) { }
finally
{
    stop.Cancel();
    await Task.WhenAll(clients);
}

[SupportedOSPlatform("windows")]
NamedPipeServerStream CreatePipe() => WindowsPipes.Create(pipeName);

async Task Relay(NamedPipeServerStream pipe)
{
    using var helper = StartHelper(false);
    using var cancel = CancellationTokenSource.CreateLinkedTokenSource(stop.Token);
    var stderr = helper.StandardError.ReadToEndAsync();
    var upstream = protocol == "raw" ? pipe.CopyToAsync(helper.StandardInput.BaseStream, cancel.Token)
        : FramedPaths.Copy(pipe, helper.StandardInput.BaseStream, toLinux: true, cancel.Token);
    var downstream = protocol == "raw" ? helper.StandardOutput.BaseStream.CopyToAsync(pipe, cancel.Token)
        : FramedPaths.Copy(helper.StandardOutput.BaseStream, pipe, toLinux: false, cancel.Token);
    try
    {
        var completed = await Task.WhenAny(upstream, downstream);
        await completed;
    }
    catch (OperationCanceledException) when (cancel.IsCancellationRequested) { }
    catch (IOException) { /* The IDE or backend disconnected. */ }
    finally
    {
        cancel.Cancel();
        helper.StandardInput.Close(); // EOF stops the Linux helper and closes its socket.
        try { await Task.WhenAll(upstream, downstream); }
        catch (OperationCanceledException) { }
        catch (IOException) { }
        using var grace = new CancellationTokenSource(TimeSpan.FromSeconds(3));
        try { await helper.WaitForExitAsync(grace.Token); }
        catch (OperationCanceledException) { helper.Kill(entireProcessTree: true); await helper.WaitForExitAsync(); }
        // Helper diagnostics contain no protocol payload or secret values.
        var diagnostic = await stderr;
        if (helper.ExitCode != 0) Console.Error.WriteLine($"Backend relay exited {helper.ExitCode}: {diagnostic.Trim()}");
    }
}
