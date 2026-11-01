using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

internal static class FramedPaths
{
    private const int MaxBody = 16 * 1024 * 1024;
    private static readonly HashSet<string> PathFields = new(StringComparer.Ordinal)
    {
        "project", "dir", "directory", "root", "roots", "path", "paths", "manifests",
        "cwd", "program", "sourcePath", "outputPath", "archivePath", "ipaPath", "dsymPath",
        // Files the user picks in the editor (an App Store Connect key, a secret's source file, the
        // IPA's destination) and build products the editor reveals.
        "p8Path", "fromFile", "out", "appPath", "profilePath", "reportPath",
    };

    // Only filesystem-valued protocol fields cross the mount mapping. Message
    // text, app arguments, environment values and debugger command strings stay intact.
    private static void Map(JsonNode? node, bool toLinux, string? field = null)
    {
        if (node is JsonObject obj)
        {
            foreach (var key in obj.Select(pair => pair.Key).ToArray())
            {
                if (key is "env" or "args" or "initCommands" or "attachCommands") continue;
                if (PathFields.Contains(key) && obj[key] is JsonValue value && value.TryGetValue<string>(out var text))
                    obj[key] = Translate(text, toLinux);
                else Map(obj[key], toLinux, key);
            }
        }
        else if (node is JsonArray array)
        {
            for (var i = 0; i < array.Count; i++)
            {
                if (field is not null && PathFields.Contains(field) && array[i] is JsonValue value && value.TryGetValue<string>(out var text))
                    array[i] = Translate(text, toLinux);
                else Map(array[i], toLinux);
            }
        }
    }

    private static string Translate(string text, bool toLinux)
    {
        if (toLinux && text.Length >= 3 && char.IsAsciiLetter(text[0]) && text[1] == ':' && text[2] is '\\' or '/')
        {
            // Normalize dot segments using Windows semantics while preserving
            // Unicode and the spelling of case-sensitive source components.
            var full = Path.GetFullPath(text);
            return $"/mnt/{char.ToLowerInvariant(full[0])}/{full[3..].Replace('\\', '/')}";
        }
        if (!toLinux && text.Length >= 7 && text.StartsWith("/mnt/", StringComparison.Ordinal)
            && char.IsAsciiLetter(text[5]) && text[6] == '/')
            return $"{char.ToUpperInvariant(text[5])}:\\{text[7..].Replace('/', '\\')}";
        return text;
    }

    // A Windows drive path as the backend sees it, or null when it has no mapping.
    internal static string? ToLinux(string path)
    {
        var linux = Translate(path, toLinux: true);
        return linux == path ? null : linux;
    }

    internal static async Task Copy(Stream input, Stream output, bool toLinux, CancellationToken cancel)
    {
        var one = new byte[1];
        while (true)
        {
            var header = new List<byte>(128);
            while (true)
            {
                var count = await input.ReadAsync(one, cancel);
                if (count == 0)
                {
                    if (header.Count != 0) throw new IOException("Truncated protocol header.");
                    return;
                }
                header.Add(one[0]);
                if (header.Count > 8192) throw new IOException("Protocol header too large.");
                var n = header.Count;
                if (n >= 4 && header[n - 4] == 13 && header[n - 3] == 10 && header[n - 2] == 13 && header[n - 1] == 10) break;
            }
            int? length = null;
            foreach (var line in Encoding.ASCII.GetString(header.ToArray()).Split("\r\n", StringSplitOptions.RemoveEmptyEntries))
            {
                var colon = line.IndexOf(':');
                if (colon < 0) throw new IOException("Malformed protocol header.");
                if (!line[..colon].Equals("Content-Length", StringComparison.OrdinalIgnoreCase)) continue;
                if (length is not null || !int.TryParse(line[(colon + 1)..].Trim(), NumberStyles.None, CultureInfo.InvariantCulture, out var parsed)
                    || parsed < 0 || parsed > MaxBody) throw new IOException("Invalid protocol frame length.");
                length = parsed;
            }
            if (length is null) throw new IOException("Missing protocol frame length.");
            var body = new byte[length.Value];
            await input.ReadExactlyAsync(body, cancel);
            var node = JsonNode.Parse(body) ?? throw new IOException("Empty protocol message.");
            Map(node, toLinux);
            var rewritten = JsonSerializer.SerializeToUtf8Bytes(node);
            if (rewritten.Length > MaxBody) throw new IOException("Mapped protocol frame too large.");
            await output.WriteAsync(Encoding.ASCII.GetBytes($"Content-Length: {rewritten.Length}\r\n\r\n"), cancel);
            await output.WriteAsync(rewritten, cancel);
            await output.FlushAsync(cancel);
        }
    }
}
