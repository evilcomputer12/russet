using System.Security.Cryptography;
using System.Text;

// Russet's engine and backend scripts in the managed distribution (/opt/russet), copied from the
// payload an installation carries (<install>\backend). The copy records the identity of the
// payload it came from in /opt/russet/.payload: the full installer replaces only the Windows side,
// so on each start the launcher copies a payload whose identity differs again. The setup page's
// backend step and the patch applier copy with the same command. Linked into Russet.Host.
internal static class BackendSync
{
    const string Stamp = "/opt/russet/.payload";

    // The installation's payload, or null where it carries none (a development installation).
    internal static string? Payload(string root) =>
        File.Exists(Path.Combine(root, "backend", "bin", "russet-dev")) ? Path.Combine(root, "backend") : null;

    // A digest of the payload's file names, sizes and write times: each build's differs, and it
    // reads no file contents.
    internal static string Id(string payload)
    {
        var listing = new StringBuilder();
        foreach (var file in Directory.EnumerateFiles(payload, "*", SearchOption.AllDirectories).Order(StringComparer.Ordinal))
        {
            var info = new FileInfo(file);
            listing.Append(Path.GetRelativePath(payload, file).Replace('\\', '/')).Append('\t')
                .Append(info.Length).Append('\t').Append(info.LastWriteTimeUtc.Ticks).Append('\n');
        }
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(listing.ToString())));
    }

    // A drive path as WSL mounts it (C:\x → /mnt/c/x), or null for any other path.
    internal static string? LinuxPath(string path)
    {
        var full = Path.GetFullPath(path);
        return full.Length >= 3 && char.IsAsciiLetter(full[0]) && full[1] == ':' && full[2] == '\\'
            ? $"/mnt/{char.ToLowerInvariant(full[0])}/{full[3..].Replace('\\', '/')}"
            : null;
    }

    // Install's exit status when it changed /etc/wsl.conf: done, and the distribution must restart
    // (wsl --terminate) for the change to apply.
    internal const int RestartExit = 10;

    // wsl.exe arguments that copy a payload (as WSL sees it) under /opt/russet as root. The
    // identity is written last, so a copy that stopped part way is copied again. It also gives the
    // distribution Russet's /etc/wsl.conf where it lacks it (the first one was its default user
    // alone): systemd, which the virtual iPhone's udev rules and module loads need, and drvfs
    // metadata, so builds can chmod and date the Windows files they write.
    internal static string[] Install(string distribution, string linuxPayload, string id) =>
    [
        "--distribution", distribution, "--user", "root", "--exec", "sh", "-c",
        $"set -e; rm -f {Stamp}; install -d -m 755 /opt/russet/bin /opt/russet/lib; " +
        "install -m 755 \"$1/bin/russet-dev\" \"$1/bin/russet-adi-host\" /opt/russet/bin/; " +
        "cp -R \"$1/lib/.\" /opt/russet/lib/; chmod 755 /opt/russet/lib/*.sh; " +
        $"install -d -m 700 -o russet -g russet /home/russet/.russet; printf '%s\\n' \"$2\" > {Stamp}; " +
        "if grep -qx 'systemd=true' /etc/wsl.conf 2>/dev/null && grep -q 'metadata' /etc/wsl.conf; then exit 0; fi; " +
        "[ ! -f /etc/wsl.conf ] || [ -f /etc/wsl.conf.russet-bak ] || cp /etc/wsl.conf /etc/wsl.conf.russet-bak; " +
        "printf '[boot]\\nsystemd=true\\n\\n[user]\\ndefault=russet\\n\\n[automount]\\noptions = \"metadata,uid=%s,gid=%s\"\\n' " +
        $"\"$(id -u russet)\" \"$(id -g russet)\" > /etc/wsl.conf; exit {RestartExit}",
        "sh", linuxPayload, id,
    ];

    // wsl.exe arguments that stop the distribution, so its next command starts it with the new /etc/wsl.conf.
    internal static string[] Restart(string distribution) => ["--terminate", distribution];

    // wsl.exe arguments that print the identity of the payload copied last (nothing before 0.6.3),
    // and fail where /etc/wsl.conf lacks what Install gives it, so that is copied again too.
    internal static string[] Recorded(string distribution) =>
        ["--distribution", distribution, "--user", "russet", "--exec", "sh", "-c",
         $"grep -qx 'systemd=true' /etc/wsl.conf && grep -q 'metadata' /etc/wsl.conf && cat {Stamp}"];

    // wsl.exe arguments that stop every Russet daemon (they all run /opt/russet/bin/russet-dev),
    // so the next start runs a newly copied engine.
    internal static string[] StopDaemons(string distribution) =>
        ["--distribution", distribution, "--user", "russet", "--exec", "pkill", "-f", "^/opt/russet/bin/russet-dev daemon run"];
}
