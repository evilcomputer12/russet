using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

// Swift for the backend: the setup page's "Swift toolchain" step and Swift's updates. The version
// is not fixed: swift.org's install API names the newest release with a build for the backend's
// Ubuntu, the download is checked with swift.org's PGP signature, and it is unpacked to
// /usr/lib/swift (where the daemon, xtool and LLDB look). The toolchain it replaces stays as
// /usr/lib/swift.previous, so an update can be rolled back; a rolled-back release is skipped.
internal static partial class Setup
{
    const string SwiftReleases = "https://www.swift.org/api/v1/install/releases.json";
    const string SwiftKeys = "https://www.swift.org/keys/all-keys.asc";
    static readonly TimeSpan SwiftListAge = TimeSpan.FromHours(6);
    // swift.org's Linux dependencies; g++ brings the C++ runtime headers Swift's C++ interop uses,
    // gnupg checks the download's signature.
    const string SwiftPackages = "binutils gcc g++ git gnupg libc6-dev libcurl4-openssl-dev libedit2 libpython3-dev libsqlite3-0 " +
        "libxml2-dev libncurses-dev libz3-dev pkg-config tzdata unzip zlib1g-dev";

    sealed record SwiftRelease(string Version, string Url);

    static string SwiftSkipFile => Path.Combine(RussetData, "swift-skip.txt");

    // The installed Swift's version, or null when there is none.
    static async Task<string?> SwiftInstalled()
    {
        var result = await Capture(["--distribution", Distribution, "--user", "russet", "--exec", "/usr/lib/swift/usr/bin/swiftc", "--version"]);
        return result.Exit == 0 && SwiftVersionLine().Match(result.Output) is { Success: true } m ? m.Groups[1].Value : null;
    }

    static async Task<string?> UbuntuVersion()
    {
        var result = await Capture(["--distribution", Distribution, "--user", "russet", "--exec", "sh", "-c",
            ". /etc/os-release; [ \"$ID\" = ubuntu ] && echo \"$VERSION_ID\""]);
        return result.Exit == 0 && UbuntuRelease().IsMatch(result.Output) ? result.Output : null;
    }

    // The newest Swift release with an x86_64 build for this backend's Ubuntu. swift.org's list is
    // kept for six hours (the IDE asks for updates every 30 s); `fresh` reads it again.
    static async Task<SwiftRelease?> SwiftLatest(bool fresh = false)
    {
        if (await UbuntuVersion() is not { } ubuntu) return null;
        var cache = Path.Combine(RussetData, "cache", "swift-releases.json");
        string? list = null;
        if (!fresh && File.Exists(cache) && DateTime.UtcNow - File.GetLastWriteTimeUtc(cache) < SwiftListAge)
            list = File.ReadAllText(cache);
        if (list is null)
        {
            try
            {
                using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(30) };
                list = await http.GetStringAsync(SwiftReleases);
                Directory.CreateDirectory(Path.GetDirectoryName(cache)!);
                File.WriteAllText(cache, list);
            }
            catch (Exception error) when (error is HttpRequestException or TaskCanceledException or IOException)
            {
                list = File.Exists(cache) ? File.ReadAllText(cache) : null;
            }
        }
        if (list is null) return null;
        try
        {
            using var releases = JsonDocument.Parse(list);
            SwiftRelease? best = null;
            Version? bestVersion = null;
            foreach (var release in releases.RootElement.EnumerateArray())
            {
                if (release.GetProperty("name").GetString() is not { } name || release.GetProperty("tag").GetString() is not { } tag
                    || !Version.TryParse(name, out var version) || (bestVersion is not null && version <= bestVersion))
                    continue;
                foreach (var platform in release.GetProperty("platforms").EnumerateArray())
                {
                    if (platform.GetProperty("name").GetString() != $"Ubuntu {ubuntu}"
                        || !platform.GetProperty("archs").EnumerateArray().Any(a => a.GetString() == "x86_64"))
                        continue;
                    var directory = platform.TryGetProperty("dir", out var dir) ? dir.GetString() : $"ubuntu{ubuntu.Replace(".", "")}";
                    best = new(name, $"https://download.swift.org/{tag.ToLowerInvariant()}/{directory}/{tag}/{tag}-ubuntu{ubuntu}.tar.gz");
                    bestVersion = version;
                }
            }
            return best;
        }
        catch (Exception error) when (error is JsonException or KeyNotFoundException or InvalidOperationException)
        {
            return null;
        }
    }

    static bool SwiftNewer(string candidate, string installed) =>
        Version.TryParse(candidate, out var a) && Version.TryParse(installed, out var b) && a > b;

    // `--setup updates`' entry for Swift, as UpdateWatcher reads it (current and upstream are versions).
    static async Task<object?> SwiftCheck()
    {
        if (await SwiftInstalled() is not { } installed || await SwiftLatest() is not { } latest) return null;
        var skipped = File.Exists(SwiftSkipFile) ? File.ReadAllText(SwiftSkipFile).Trim() : "";
        return new
        {
            current = installed, upstream = latest.Version, update = SwiftNewer(latest.Version, installed),
            staged = skipped == latest.Version ? "skipped" : "none", reason = "", running = false,
        };
    }

    static async Task<int> InstallSwift(string step, Action<object> emit, Func<string, string, bool, int> fail, SwiftRelease release)
    {
        void Progress(string message, double fraction) => emit(new { step, @event = "progress", message, fraction });
        var data = Path.Combine(RussetData, "Backends", Distribution);
        Directory.CreateDirectory(data);
        if (new DriveInfo(Path.GetPathRoot(data)!).AvailableFreeSpace < (8L << 30))
            return fail("space", "Swift needs about 8 GB free on this drive (the download, the toolchain and the previous one).", true);
        var tarball = Path.Combine(data, Path.GetFileName(new Uri(release.Url).AbsolutePath));
        var signature = tarball + ".sig";
        var keys = Path.Combine(data, "swift-keys.asc");
        Progress($"Downloading Swift {release.Version}", 0);
        if (!await Download(release.Url, tarball, fraction => Progress($"Downloading Swift {release.Version}", fraction * 0.6))
            || !await Download(release.Url + ".sig", signature, _ => { }) || !await Download(SwiftKeys, keys, _ => { }))
            return fail("download", "Swift did not download; check the connection, then retry.", true);
        if (FramedPaths.ToLinux(tarball) is not { } linuxTarball || FramedPaths.ToLinux(keys) is not { } linuxKeys)
            return fail("location", "Russet's data folder is not readable from WSL.", false);
        try
        {
            Progress("Installing the libraries Swift needs (apt)", 0.65);
            var packages = await Capture(["--distribution", Distribution, "--user", "root", "--exec", "sh", "-c",
                $"export DEBIAN_FRONTEND=noninteractive; apt-get update -qq && apt-get install -y -qq --no-install-recommends {SwiftPackages}"]);
            if (packages.Exit != 0)
                return fail("packages", "Ubuntu could not install the libraries Swift needs (apt-get); check the connection, then retry.", true);
            Progress($"Checking Swift {release.Version}'s signature", 0.75);
            var verified = await Capture(["--distribution", Distribution, "--user", "russet", "--exec", "sh", "-c",
                "set -e; g=$(mktemp -d); export GNUPGHOME=\"$g\"; gpg --batch --quiet --import \"$2\" 2>/dev/null; " +
                "gpg --batch --verify \"$1.sig\" \"$1\"; rm -rf \"$g\"", "sh", linuxTarball, linuxKeys]);
            if (verified.Exit != 0)
                return fail("signature", "The Swift download is not signed by swift.org's keys; nothing was installed.", true);
            Progress($"Unpacking Swift {release.Version}", 0.8);
            // Unpacked beside the target first; the toolchain in use moves aside only once that worked.
            var unpack = await Capture(["--distribution", Distribution, "--user", "root", "--exec", "sh", "-c",
                "set -e; rm -rf /usr/lib/swift.partial; mkdir -p /usr/lib/swift.partial; " +
                "tar -xzf \"$1\" -C /usr/lib/swift.partial --strip-components=1; " +
                "rm -rf /usr/lib/swift.previous; if [ -d /usr/lib/swift ]; then mv /usr/lib/swift /usr/lib/swift.previous; fi; " +
                "mv /usr/lib/swift.partial /usr/lib/swift", "sh", linuxTarball]);
            if (unpack.Exit != 0) return fail("unpack", "Swift could not be unpacked into WSL.", true);
            if (await SwiftInstalled() is not { } version)
                return fail("verify", "Swift was installed but swiftc does not run; roll back, or repair the backend.", true);
            emit(new { step, @event = "log", message = $"Swift {version} is installed." });
            return 0;
        }
        finally
        {
            foreach (var file in new[] { tarball, signature, keys }) File.Delete(file);
        }
    }

    // swift-update / swift-retry-update / swift-rollback, which UpdateWatcher runs.
    static async Task<int> RunSwiftMaintenance(string id)
    {
        void Emit(object value) => Console.WriteLine(JsonSerializer.Serialize(value));
        int Fail(string reason, string message, bool retryable)
        {
            Emit(new { step = id, @event = "failed", code = $"setup.{id}.{reason}", message, retryable });
            return 1;
        }
        if (id == "swift-rollback")
        {
            var current = await SwiftInstalled();
            var swap = await Capture(["--distribution", Distribution, "--user", "root", "--exec", "sh", "-c",
                "set -e; [ -d /usr/lib/swift.previous ]; rm -rf /usr/lib/swift.next; mv /usr/lib/swift /usr/lib/swift.next; " +
                "mv /usr/lib/swift.previous /usr/lib/swift; mv /usr/lib/swift.next /usr/lib/swift.previous"]);
            if (swap.Exit != 0) return Fail("rollback", "There is no previous Swift toolchain to roll back to.", false);
            // The release rolled back from is not offered again.
            if (current is not null) File.WriteAllText(SwiftSkipFile, current);
            Emit(new { step = id, @event = "done" });
            return 0;
        }
        if (await SwiftLatest(fresh: true) is not { } latest)
            return Fail("lookup", "swift.org's release list is unreachable; Swift stays as it is.", true);
        if (await InstallSwift(id, Emit, Fail, latest) != 0) return 1;
        Emit(new { step = id, @event = "done" });
        return 0;
    }

    [GeneratedRegex(@"Swift version (\d+\.\d+(?:\.\d+)?)")]
    private static partial Regex SwiftVersionLine();
    [GeneratedRegex(@"^\d{2}\.\d{2}$")]
    private static partial Regex UbuntuRelease();
}
