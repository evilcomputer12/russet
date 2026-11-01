using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

// The backend's secret-store passphrase: random, kept only as a DPAPI blob bound to this
// Windows user, and handed to the Linux daemon through the environment (WSLENV), never
// on a command line. Without it the store's sealed Apple ID session cannot be opened.
internal static class BackendSecrets
{
    const string FileName = "backend-secrets.dpapi";
    static readonly byte[] Entropy = Encoding.UTF8.GetBytes("Russet IDE backend secret store v1");

    internal static string Passphrase(string profile)
    {
        var path = Path.Combine(profile, FileName);
        if (File.Exists(path))
            return Encoding.UTF8.GetString(Unprotect(File.ReadAllBytes(path)));
        var passphrase = Convert.ToBase64String(RandomNumberGenerator.GetBytes(32));
        var temporary = path + ".partial";
        File.WriteAllBytes(temporary, Protect(Encoding.UTF8.GetBytes(passphrase)));
        File.Move(temporary, path);
        return passphrase;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct Blob { public int Size; public nint Data; }

    const int UiForbidden = 0x1;

    [DllImport("crypt32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CryptProtectData(ref Blob input, string? description, ref Blob entropy, nint reserved, nint prompt, int flags, out Blob output);

    [DllImport("crypt32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CryptUnprotectData(ref Blob input, nint description, ref Blob entropy, nint reserved, nint prompt, int flags, out Blob output);

    [DllImport("kernel32.dll")]
    static extern nint LocalFree(nint memory);

    static byte[] Protect(byte[] data) => Transform(data, protect: true);
    static byte[] Unprotect(byte[] data) => Transform(data, protect: false);

    static byte[] Transform(byte[] data, bool protect)
    {
        var input = GCHandle.Alloc(data, GCHandleType.Pinned);
        var salt = GCHandle.Alloc(Entropy, GCHandleType.Pinned);
        try
        {
            var inBlob = new Blob { Size = data.Length, Data = input.AddrOfPinnedObject() };
            var entropy = new Blob { Size = Entropy.Length, Data = salt.AddrOfPinnedObject() };
            var ok = protect
                ? CryptProtectData(ref inBlob, "Russet backend secrets", ref entropy, 0, 0, UiForbidden, out var output)
                : CryptUnprotectData(ref inBlob, 0, ref entropy, 0, 0, UiForbidden, out output);
            if (!ok) throw new IOException("Windows could not protect the Russet backend's secret store key.");
            try
            {
                var result = new byte[output.Size];
                Marshal.Copy(output.Data, result, 0, output.Size);
                return result;
            }
            finally { LocalFree(output.Data); }
        }
        finally { input.Free(); salt.Free(); }
    }
}
