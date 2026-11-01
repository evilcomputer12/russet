using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;

// The setup page's "Russet backend" step on a release installation: Ubuntu in WSL as the managed
// Russet-Dev distribution, then the engine and backend scripts the installer carries
// (<install>\backend\bin and \lib) under /opt/russet, then the daemon. A development
// installation has no payload and keeps update-dev-install.ps1.
internal static partial class Setup
{
    // Ubuntu 26.04's WSL image, pinned (scripts/prepare-backend.ps1 uses the same).
    const string RootfsUrl = "https://releases.ubuntu.com/resolute/ubuntu-26.04-wsl-amd64.wsl";
    const string RootfsSha256 = "96c7f5fb28a7fe28245331f9bfbe4375f18dd29a4850116ad3c4f60f6700c55c";
    const long BackendFreeBytes = 25L << 30;

    static string? InstallRoot => Environment.GetEnvironmentVariable("RUSSET_INSTALL_ROOT") is { Length: > 0 } root ? root
        : Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, ".."));

    static string? BackendPayload() =>
        InstallRoot is { } root && File.Exists(Path.Combine(root, "backend", "bin", "russet-dev")) ? Path.Combine(root, "backend") : null;

    static async Task<int> InstallBackend(Action<object> emit, Func<string, string, bool, int> fail)
    {
        void Progress(string message, double? fraction = null) =>
            emit(fraction is { } f ? new { step = "backend", @event = "progress", message, fraction = f } : new { step = "backend", @event = "progress", message });
        if (BackendPayload() is not { } payload)
            return fail("missing", "This installation carries no backend. Install Russet from its GitHub release, then retry.", false);
        if (FramedPaths.ToLinux(payload) is not { } linuxPayload)
            return fail("location", "Install Russet into a folder WSL can read, then retry.", false);
        var names = (await Capture(["--list", "--quiet"], Encoding.Unicode)).Output.Split('\n', StringSplitOptions.TrimEntries);
        var data = Path.Combine(RussetData, "Backends", "Russet");
        if (!names.Contains(Distribution))
        {
            Directory.CreateDirectory(data);
            if (new DriveInfo(Path.GetPathRoot(data)!).AvailableFreeSpace < BackendFreeBytes)
                return fail("space", "Russet's backend needs 25 GB free on this drive.", true);
            var rootfs = Path.Combine(data, "ubuntu-26.04-wsl-amd64.wsl");
            if (!File.Exists(rootfs) || !await HashMatches(rootfs))
            {
                Progress("Downloading Ubuntu for WSL", 0);
                if (!await Download(RootfsUrl, rootfs, fraction => Progress("Downloading Ubuntu for WSL", fraction * 0.6)))
                    return fail("download", "Ubuntu for WSL did not download; check the connection, then retry.", true);
                if (!await HashMatches(rootfs))
                {
                    File.Delete(rootfs);
                    return fail("checksum", "The Ubuntu download did not match its checksum; retry.", true);
                }
            }
            Progress("Creating Russet's WSL distribution", 0.65);
            if ((await Capture(["--import", Distribution, Path.Combine(data, "distro"), rootfs, "--version", "2"], Encoding.Unicode)).Exit != 0)
                return fail("import", "WSL could not create Russet's distribution.", true);
            if ((await Capture(["--distribution", Distribution, "--user", "root", "--exec", "sh", "-c",
                    "id russet >/dev/null 2>&1 || useradd --create-home --shell /bin/bash russet; " +
                    "printf '[user]\\ndefault=russet\\n' > /etc/wsl.conf"])).Exit != 0)
                return fail("user", "Russet's backend user could not be created.", true);
            File.Delete(rootfs);
        }
        Progress("Installing Russet's engine", 0.8);
        if (!await InstallPayload(linuxPayload)) return fail("engine", "Russet's engine could not be installed into WSL.", true);
        Progress("Starting Russet's engine", 0.95);
        if (!await StartDaemon()) return fail("daemon", "Russet's engine was installed but did not start; restart Russet.", true);
        return 0;
    }

    // The engine and backend scripts (a payload folder as WSL sees it) under /opt/russet. A
    // running daemon keeps its old binary until it stops; the patch applier stops it first.
    internal static async Task<bool> InstallPayload(string linuxPayload) =>
        (await Capture(["--distribution", Distribution, "--user", "root", "--exec", "sh", "-c",
            "set -e; install -d -m 755 /opt/russet/bin /opt/russet/lib; " +
            "install -m 755 \"$1/bin/russet-dev\" \"$1/bin/russet-adi-host\" /opt/russet/bin/; " +
            "cp -R \"$1/lib/.\" /opt/russet/lib/; chmod 755 /opt/russet/lib/*.sh; " +
            "install -d -m 700 -o russet -g russet /home/russet/.russet", "sh", linuxPayload])).Exit == 0;

    // Stops every Russet daemon in the backend (they all run /opt/russet/bin/russet-dev), so the
    // next start runs a newly installed engine.
    internal static async Task StopDaemons() =>
        await Capture(["--distribution", Distribution, "--user", "russet", "--exec", "pkill", "-f", "^/opt/russet/bin/russet-dev daemon run"]);

    static async Task<bool> HashMatches(string file)
    {
        await using var stream = File.OpenRead(file);
        return Convert.ToHexStringLower(await SHA256.HashDataAsync(stream)) == RootfsSha256;
    }

    static async Task<bool> Download(string url, string destination, Action<double> progress)
    {
        try
        {
            using var http = new HttpClient { Timeout = TimeSpan.FromHours(1) };
            using var response = await http.GetAsync(url, HttpCompletionOption.ResponseHeadersRead);
            response.EnsureSuccessStatusCode();
            var total = response.Content.Headers.ContentLength ?? 0;
            var partial = destination + ".partial";
            await using (var output = File.Create(partial))
            await using (var input = await response.Content.ReadAsStreamAsync())
            {
                var buffer = new byte[1 << 20];
                long done = 0, reported = 0;
                int read;
                while ((read = await input.ReadAsync(buffer)) > 0)
                {
                    await output.WriteAsync(buffer.AsMemory(0, read));
                    done += read;
                    if (total > 0 && done - reported > total / 50) { reported = done; progress((double)done / total); }
                }
            }
            File.Move(partial, destination, overwrite: true);
            return true;
        }
        catch (Exception error) when (error is HttpRequestException or IOException or TaskCanceledException)
        {
            return false;
        }
    }

    // The daemon as the launcher starts it, for a backend installed after the editor opened.
    static async Task<bool> StartDaemon()
    {
        var profile = Environment.GetEnvironmentVariable("RUSSET_IDE_PROFILE");
        var home = Environment.GetEnvironmentVariable("RUSSET_BACKEND_HOME");
        if (string.IsNullOrEmpty(profile) || string.IsNullOrEmpty(home) || !home.StartsWith("/home/russet/", StringComparison.Ordinal)) return false;
        var start = new ProcessStartInfo(Wsl) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
        start.Environment["RUSSET_SECRETS_PASSPHRASE"] = BackendSecrets.Passphrase(profile);
        var forwarded = Environment.GetEnvironmentVariable("WSLENV");
        start.Environment["WSLENV"] = string.IsNullOrEmpty(forwarded) ? "RUSSET_SECRETS_PASSPHRASE" : forwarded + ":RUSSET_SECRETS_PASSPHRASE";
        foreach (var argument in new[] { "--distribution", Distribution, "--user", "russet", "--exec", "env", $"RUSSET_HOME={home}",
                     "RUSSET_DEVICE_BACKEND=none", "RUSSET_SECRETS_BACKEND=passphrase", "PATH=/usr/lib/swift/usr/bin:/usr/local/bin:/usr/bin:/bin",
                     "/opt/russet/bin/russet-dev", "daemon", "start", "--idle-timeout", "10m", "--json" })
            start.ArgumentList.Add(argument);
        using var daemon = Process.Start(start);
        if (daemon is null) return false;
        var output = daemon.StandardOutput.ReadToEndAsync();
        var errors = daemon.StandardError.ReadToEndAsync();
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        try { await daemon.WaitForExitAsync(deadline.Token); }
        catch (OperationCanceledException) { daemon.Kill(entireProcessTree: true); return false; }
        await Task.WhenAll(output, errors);
        return daemon.ExitCode == 0;
    }
}
