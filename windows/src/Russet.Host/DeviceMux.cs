using System.Net;
using System.Net.Sockets;

internal static class DeviceMux
{
    // Called through WSL interop by the managed same-user Unix mux listener.
    // Fixed Windows loopback destination; never exposes AMDS to a network.
    internal static async Task Run()
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        using var socket = new TcpClient(AddressFamily.InterNetwork);
        await socket.ConnectAsync(IPAddress.Loopback, 27015, deadline.Token);
        using var stream = socket.GetStream();
        using var stop = new CancellationTokenSource();
        var upstream = Console.OpenStandardInput().CopyToAsync(stream, stop.Token);
        var downstream = stream.CopyToAsync(Console.OpenStandardOutput(), stop.Token);
        try { await await Task.WhenAny(upstream, downstream); }
        finally
        {
            stop.Cancel();
            socket.Close();
            try { await Task.WhenAll(upstream, downstream); }
            catch (OperationCanceledException) { }
            catch (IOException) { }
        }
    }
}
