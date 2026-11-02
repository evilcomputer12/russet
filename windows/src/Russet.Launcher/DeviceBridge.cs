using System.Diagnostics;

// Physical iPhones: Apple Devices' usbmux on the Windows loopback reaches the backend through
// Russet's mux helper (/opt/russet/lib/device_mux.py), a same-user Unix socket in the
// backend's runtime folder. The helper lives exactly as long as its standard input, which
// the launcher holds open while the editor runs.
internal static class DeviceBridge
{
    // 0: a bridge already serves the socket; 1: a stale socket was removed; 2: none.
    const string Probe = """
        import os, socket, sys
        path = sys.argv[1]
        if not os.path.exists(path): sys.exit(2)
        probe = socket.socket(socket.AF_UNIX)
        try:
            probe.connect(path); sys.exit(0)
        except OSError:
            os.unlink(path); sys.exit(1)
        """;

    /// <summary>True when devices are reachable; <paramref name="owned"/> is the bridge this launcher must close.</summary>
    internal static bool Start(string wsl, string host, string socket, out Process? owned)
    {
        owned = null;
        if (Run(wsl, "python3", "-I", "-c", Probe, socket) == 0) return true;
        var start = Backend(wsl, "python3", "-I", "/opt/russet/lib/device_mux.py", "--socket", socket, "--host", ToLinux(host));
        start.RedirectStandardInput = true;
        var bridge = Process.Start(start);
        if (bridge is null) return false;
        _ = bridge.StandardError.ReadToEndAsync();
        try
        {
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(20));
            if (bridge.StandardOutput.ReadLineAsync(deadline.Token).AsTask().GetAwaiter().GetResult() == "ready")
            {
                owned = bridge;
                return true;
            }
        }
        catch (OperationCanceledException) { }
        bridge.Kill(entireProcessTree: true);
        bridge.Dispose();
        // Russet started twice in a row: the other launcher's bridge took the socket first, and the helper refuses
        // to replace it. Use that bridge; answering "no phones" here would restart the daemon without them.
        // Otherwise Apple Devices is missing or the backend cannot run the helper: the IDE opens without phones.
        return Run(wsl, "python3", "-I", "-c", Probe, socket) == 0;
    }

    /// <summary>EOF on the helper's input closes its socket and connections.</summary>
    internal static void Stop(Process bridge)
    {
        bridge.StandardInput.Close();
        if (!bridge.WaitForExit(5000)) bridge.Kill(entireProcessTree: true);
        bridge.Dispose();
    }

    static ProcessStartInfo Backend(string wsl, params string[] command)
    {
        var start = new ProcessStartInfo(wsl) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var value in new[] { "-d", "Russet-Dev", "-u", "russet", "--cd", "~", "--exec" }.Concat(command)) start.ArgumentList.Add(value);
        return start;
    }

    static int Run(string wsl, params string[] command)
    {
        using var process = Process.Start(Backend(wsl, command)) ?? throw new IOException("Cannot start the Russet backend.");
        _ = process.StandardOutput.ReadToEndAsync();
        _ = process.StandardError.ReadToEndAsync();
        if (!process.WaitForExit(15000)) { process.Kill(entireProcessTree: true); return -1; }
        return process.ExitCode;
    }

    static string ToLinux(string windowsPath) =>
        $"/mnt/{char.ToLowerInvariant(windowsPath[0])}{windowsPath[2..].Replace('\\', '/')}";
}
