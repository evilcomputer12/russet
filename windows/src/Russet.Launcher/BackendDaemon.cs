using System.Diagnostics;

// The engine's daemon in WSL, started the same way by the launcher when Russet opens, by the setup
// page once it installs the engine, and by the IDE when the daemon has stopped while Russet is open
// (Russet.Host --setup daemon-start).
internal static class BackendDaemon
{
    // Held while a setup step has the daemons stopped on purpose (installing the engine), so that no
    // start from the IDE runs the engine being replaced. Its existence is the signal; nobody waits on it.
    internal const string MaintenanceMutex = @"Local\Russet-Backend-Maintenance";

    internal static string MuxSocket(string home) => home + "/runtime/host-usbmuxd.sock";

    // wsl.exe running `russet-dev daemon start` for the backend home `home`, which returns once the
    // daemon answers (at once when one runs already). Physical iPhones come through the Apple Devices
    // bridge when `devices`: a development engine admits the live backend only with RUSSET_LANE=A, and
    // its host-wide device lock arbitrates with other users.
    internal static ProcessStartInfo Start(string wsl, string home, string profile, bool devices)
    {
        var start = new ProcessStartInfo(wsl)
        {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true,
        };
        // The sign-in survives restarts in the backend's passphrase store; WSLENV forwards the
        // passphrase into the Linux environment so it never appears in an argument list.
        start.Environment["RUSSET_SECRETS_PASSPHRASE"] = BackendSecrets.Passphrase(profile);
        var forwarded = Environment.GetEnvironmentVariable("WSLENV");
        start.Environment["WSLENV"] = string.IsNullOrEmpty(forwarded) ? "RUSSET_SECRETS_PASSPHRASE" : forwarded + ":RUSSET_SECRETS_PASSPHRASE";
        string[] deviceEnvironment = devices
            ? ["RUSSET_DEVICE_BACKEND=live", "RUSSET_LANE=A", $"USBMUXD_SOCKET_ADDRESS={MuxSocket(home)}"]
            : ["RUSSET_DEVICE_BACKEND=none"];
        foreach (var value in new[] { "-d", "Russet-Dev", "-u", "russet", "--exec", "env", $"RUSSET_HOME={home}" }
            .Concat(deviceEnvironment)
            .Concat(["RUSSET_SECRETS_BACKEND=passphrase", "PATH=/usr/lib/swift/usr/bin:/usr/local/bin:/usr/bin:/bin",
            // Development build: Apple sign-in stages, header names and Apple's status codes, never values.
            "RUSSET_LOG=info,russet_gsa=debug,hyper=warn",
            "/opt/russet/bin/russet-dev", "daemon", "start", "--idle-timeout", "10m", "--json"])) start.ArgumentList.Add(value);
        return start;
    }
}
