using System.ComponentModel;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Security.Principal;
using System.Security.AccessControl;
using Microsoft.Win32.SafeHandles;

[SupportedOSPlatform("windows")]
internal static class WindowsPipes
{
    [StructLayout(LayoutKind.Sequential)]
    private struct SecurityAttributes
    {
        internal uint Length;
        internal nint Descriptor;
        [MarshalAs(UnmanagedType.Bool)] internal bool Inherit;
    }

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(
        string descriptor, uint revision, out nint securityDescriptor, out uint size);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafePipeHandle CreateNamedPipeW(string name, uint openMode, uint pipeMode,
        uint maxInstances, uint outputBuffer, uint inputBuffer, uint timeout, ref SecurityAttributes attributes);

    [DllImport("kernel32.dll")]
    private static extern nint LocalFree(nint memory);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern uint GetSecurityInfo(SafePipeHandle handle, uint objectType, uint securityInformation,
        out nint owner, out nint group, out nint dacl, out nint sacl, out nint descriptor);

    [DllImport("advapi32.dll")]
    private static extern uint GetSecurityDescriptorLength(nint descriptor);

    private static void VerifyAccess(SafePipeHandle handle, string sid)
    {
        // Read back the kernel object's ACL, rather than attesting only to the
        // descriptor we asked Windows to apply.
        var error = GetSecurityInfo(handle, 6, 4, out _, out _, out _, out _, out var descriptor);
        if (error != 0) throw new Win32Exception((int)error);
        try
        {
            var bytes = new byte[GetSecurityDescriptorLength(descriptor)];
            Marshal.Copy(descriptor, bytes, 0, bytes.Length);
            var actual = new RawSecurityDescriptor(bytes, 0);
            var allowed = new HashSet<string>(StringComparer.Ordinal) { sid, "S-1-5-18" };
            var found = new HashSet<string>(StringComparer.Ordinal);
            if (!actual.ControlFlags.HasFlag(ControlFlags.DiscretionaryAclProtected) || actual.DiscretionaryAcl is null)
                throw new IOException("Host pipe access control verification failed.");
            foreach (GenericAce entry in actual.DiscretionaryAcl)
            {
                if (entry is not CommonAce ace || ace.AceQualifier != AceQualifier.AccessAllowed
                    || !allowed.Contains(ace.SecurityIdentifier.Value))
                    throw new IOException("Host pipe access control verification failed.");
                found.Add(ace.SecurityIdentifier.Value);
            }
            if (!found.SetEquals(allowed)) throw new IOException("Host pipe access control verification failed.");
        }
        finally { LocalFree(descriptor); }
    }

    internal static NamedPipeServerStream Create(string name)
    {
        using var identity = WindowsIdentity.GetCurrent();
        var sid = identity.User?.Value ?? throw new IOException("Cannot establish host user identity.");
        // Match the engine's user/SYSTEM-only protected DACL. Reject remote SMB
        // pipe clients at the kernel boundary, including same-account clients.
        var sddl = $"D:P(A;;GA;;;{sid})(A;;GA;;;SY)";
        if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, out var descriptor, out _))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            var security = new SecurityAttributes { Length = (uint)Marshal.SizeOf<SecurityAttributes>(), Descriptor = descriptor, Inherit = false };
            const uint duplex = 0x3, overlapped = 0x40000000, firstInstance = 0x80000, rejectRemote = 0x8;
            var handle = CreateNamedPipeW(@"\\.\pipe\" + name, duplex | overlapped | firstInstance,
                rejectRemote, 1, 65536, 65536, 0, ref security);
            if (handle.IsInvalid)
            {
                var error = Marshal.GetLastWin32Error();
                handle.Dispose();
                throw new Win32Exception(error);
            }
            try
            {
                VerifyAccess(handle, sid);
                return new NamedPipeServerStream(PipeDirection.InOut, true, false, handle);
            }
            catch { handle.Dispose(); throw; }
        }
        finally { LocalFree(descriptor); }
    }
}
