using System.Net;
using System.Net.Http.Headers;

// The setup page's downloads (Ubuntu for WSL, Swift), made for slow and flaky connections. A file
// arrives in <destination>.partial, which outlives a failed attempt and a failed step: the next
// attempt, or the step's retry, asks for the rest (an HTTP range). No overall time limit: a slow
// connection takes as long as it takes, and an attempt that receives nothing for Stall fails.
internal static class Downloads
{
    // How long an attempt may receive nothing, and how many attempts in a row may fail without a
    // byte before Get gives up (about four minutes of waits between them). Settable for tests.
    internal static TimeSpan Stall = TimeSpan.FromMinutes(2);
    internal static int Attempts = 6;
    internal static Func<int, TimeSpan> Wait = failures => TimeSpan.FromSeconds(Math.Min(60, 5 << failures));

    // From this size a partial download resumes. A range of a compressed response (swift.org sends
    // its PGP keys gzip-encoded) would not decode, and a smaller file is quicker fetched again.
    const long ResumeFrom = 1 << 20;

    // Decodes a gzip or brotli Content-Encoding: swift.org sends its PGP keys gzip-encoded whatever
    // the request asks for, and the saved file must be the keys themselves.
    internal static HttpClient Http(TimeSpan timeout) =>
        new(new HttpClientHandler { AutomaticDecompression = DecompressionMethods.All }) { Timeout = timeout };

    // Downloads url to destination: true once it is all there. Attempts repeat while each brings
    // bytes; a server that ignores the range sends the whole file again; an answer such as 404
    // ends at once.
    internal static async Task<bool> Get(string url, string destination, Action<double> progress)
    {
        var partial = destination + ".partial";
        using var http = Http(Timeout.InfiniteTimeSpan);
        for (var failures = 0; failures < Attempts;)
        {
            if (failures > 0) await Task.Delay(Wait(failures));
            var have = File.Exists(partial) ? new FileInfo(partial).Length : 0;
            if (have < ResumeFrom) have = 0;
            long received = 0;
            try
            {
                using var request = new HttpRequestMessage(HttpMethod.Get, url);
                if (have > 0) request.Headers.Range = new RangeHeaderValue(have, null);
                using var stall = new CancellationTokenSource(Stall);
                using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, stall.Token);
                if (response.StatusCode == HttpStatusCode.RequestedRangeNotSatisfiable)
                {
                    // Nothing past the partial's end: it is the whole file, or another file.
                    if (have > 0 && response.Content.Headers.ContentRange?.Length == have)
                    {
                        File.Move(partial, destination, overwrite: true);
                        return true;
                    }
                    File.Delete(partial);
                    failures++;
                    continue;
                }
                if ((int)response.StatusCode is >= 400 and < 500 and not 408 and not 429) return false;
                response.EnsureSuccessStatusCode();
                var resumed = have > 0 && response.StatusCode == HttpStatusCode.PartialContent;
                if (!resumed) have = 0;
                var total = response.Content.Headers.ContentLength is { } length ? have + length : 0;
                await using (var output = new FileStream(partial, resumed ? FileMode.Append : FileMode.Create, FileAccess.Write))
                await using (var input = await response.Content.ReadAsStreamAsync(stall.Token))
                {
                    var buffer = new byte[1 << 20];
                    var reported = have;
                    while (true)
                    {
                        stall.CancelAfter(Stall);
                        var read = await input.ReadAsync(buffer, stall.Token);
                        if (read == 0) break;
                        await output.WriteAsync(buffer.AsMemory(0, read), CancellationToken.None);
                        have += read;
                        received += read;
                        if (total > 0 && have - reported > total / 50) { reported = have; progress((double)have / total); }
                    }
                }
                if (total > 0 && have < total) throw new IOException("The connection closed before the end of the file.");
                File.Move(partial, destination, overwrite: true);
                return true;
            }
            catch (Exception error) when (error is HttpRequestException or IOException or OperationCanceledException)
            {
                // An attempt that brought bytes starts the count again: a slow, flaky connection gets there.
                failures = received > 0 ? 1 : failures + 1;
            }
        }
        return false;
    }
}
