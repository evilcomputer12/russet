using System.Diagnostics;
using System.Text;
using System.Text.Json;

// Where the backend's WSL disk lives, which the owner chooses from the IDE ("Russet: Move WSL Disk
// to Another Drive"): `--setup run wsl-move --arg folder=<folder>`. With the backend installed,
// Russet closes and move-wsl-disk.ps1 (beside the host; it also runs on its own) moves the disk
// to <folder>\Russet-Dev\distro with `wsl --manage --move`, in a window of its own, then reopens
// Russet. Before the backend exists, the backend step creates it there. The setup page's
// downloads (Ubuntu's image, Swift's) go beside the disk, in <folder>\Russet-Dev.
internal static partial class Setup
{
    static readonly string BackendChoiceFile = Path.Combine(RussetData, "backend.json");

    // Russet's folder for the backend: <folder>\distro holds the disk where Russet created or moved it.
    static string BackendData()
    {
        var fallback = Path.Combine(RussetData, "Backends", Distribution);
        if (Vhdx() is { } disk) return disk.Directory is { Name: "distro", Parent: { } parent } ? parent.FullName : fallback;
        try
        {
            using var choice = JsonDocument.Parse(File.ReadAllText(BackendChoiceFile));
            return choice.RootElement.GetProperty("folder").GetString() is { Length: > 0 } chosen ? chosen : fallback;
        }
        catch (Exception error) when (error is IOException or JsonException or KeyNotFoundException or InvalidOperationException)
        {
            return fallback;
        }
    }

    static async Task<int> RunMove(string[] args)
    {
        void Emit(object value) => Console.WriteLine(JsonSerializer.Serialize(value));
        int Fail(string reason, string message, bool retryable = false)
        {
            Emit(new { step = "wsl-move", @event = "failed", code = $"setup.wsl-move.{reason}", message, retryable });
            return 1;
        }
        if (args is not ["--arg", var pair] || pair.Split('=', 2) is not ["folder", { Length: > 0 } folder] || !Path.IsPathFullyQualified(folder))
            return Usage();
        folder = Path.GetFullPath(folder);
        var data = Path.Combine(folder, Distribution);
        if (Vhdx() is null)
        {
            // Not installed yet: the backend step creates the disk there.
            var drive = new DriveInfo(Path.GetPathRoot(folder)!);
            if (!drive.IsReady || drive.DriveType != DriveType.Fixed || drive.DriveFormat is not ("NTFS" or "ReFS"))
                return Fail("drive", $"{drive.Name} is not a local NTFS drive; WSL keeps its disks on those.");
            Directory.CreateDirectory(RussetData);
            File.WriteAllText(BackendChoiceFile, JsonSerializer.Serialize(new { version = 1, folder = data }));
            Emit(new { step = "wsl-move", @event = "progress", message = $"Russet's backend will be created in {data}." });
            Emit(new { step = "wsl-move", @event = "done" });
            return 0;
        }
        var install = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, ".."));
        var launcher = Path.Combine(install, "Russet.Launcher.exe");
        var script = Path.Combine(AppContext.BaseDirectory, "move-wsl-disk.ps1");
        if (!File.Exists(launcher) || !File.Exists(script))
            return Fail("missing", "This installation cannot move the disk itself; reinstall Russet from its GitHub release, then retry.");
        if (Creating(await Component("vphone_component.sh"))) return Fail("busy", StillCreating, true);
        // The script says whether the move can start (drive, space) before Russet closes for it.
        var check = await PowerShell(MoveCommand(script, folder, check: true), visible: false);
        if (check.Exit != 0) return Fail("check", check.Output.Length > 0 ? check.Output : "The disk cannot move there.");
        if ((await PowerShell(MoveCommand(script, folder, check: false, install, launcher), visible: true)).Exit != 0)
            return Fail("start", "The move could not start; move the disk with move-wsl-disk.ps1 instead.", true);
        Emit(new { step = "wsl-move", @event = "done" });
        return 0;
    }

    // A command that runs move-wsl-disk.ps1 (beside the host) from its text: a command, which no
    // execution policy stops as it would the file.
    static string MoveCommand(string script, string folder, bool check, string? install = null, string? launcher = null)
    {
        static string Quote(string text) => "'" + text.Replace("'", "''") + "'";
        var command = new StringBuilder($"& ([ScriptBlock]::Create([IO.File]::ReadAllText({Quote(script)}))) -Folder {Quote(folder)} -Distribution {Quote(Distribution)}");
        if (check) command.Append(" -Check");
        if (install is not null) command.Append($" -WaitFor {Quote(install)}");
        if (launcher is not null) command.Append($" -Reopen {Quote(launcher)}");
        return command.Append("; exit $LASTEXITCODE").ToString();
    }

    // Runs a PowerShell command hidden, for its output, or in a window of its own (where the
    // script waits for Enter after a failed move), without waiting.
    static async Task<(int Exit, string Output)> PowerShell(string command, bool visible)
    {
        var start = new ProcessStartInfo(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "WindowsPowerShell", "v1.0", "powershell.exe"))
        {
            Arguments = (visible ? "-NoProfile " : "-NoProfile -NonInteractive ") + "-EncodedCommand " + Convert.ToBase64String(Encoding.Unicode.GetBytes(command)),
            UseShellExecute = visible, CreateNoWindow = !visible, RedirectStandardOutput = !visible, RedirectStandardError = !visible,
        };
        if (visible)
        {
            using var window = Process.Start(start);
            return (window is null ? 1 : 0, "");
        }
        using var process = Process.Start(start) ?? throw new IOException("Cannot start PowerShell.");
        var output = process.StandardOutput.ReadToEndAsync();
        _ = process.StandardError.ReadToEndAsync();
        await process.WaitForExitAsync();
        return (process.ExitCode, (await output).Trim());
    }
}
