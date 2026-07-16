using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;

namespace UbarShell;

internal sealed class EngineBridge : IDisposable
{
    private const uint AbiV1 = 1;
    private nint library;
    private ulong profile;
    private EngineApi api;

    [StructLayout(LayoutKind.Sequential)]
    private struct Bytes { public nint Data; public nuint Length; }

    [StructLayout(LayoutKind.Sequential)]
    private struct ProfileConfig
    {
        public uint Size;
        public uint Kind;
        public Bytes DataDirectory;
        public Bytes CacheDirectory;
        public ulong MemoryTarget;
        public ulong MemoryCeiling;
        [MarshalAs(UnmanagedType.I1)] public bool PartitionStorage;
        [MarshalAs(UnmanagedType.I1)] public bool BlockThirdPartyCookies;
        [MarshalAs(UnmanagedType.I1)] public bool RequireSandbox;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ViewConfig
    {
        public uint Size;
        public nint NativeParent;
        public uint Width;
        public uint Height;
        public double DeviceScale;
        [MarshalAs(UnmanagedType.I1)] public bool Visible;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Callbacks { public uint Size; public nint UserData; public nint Event; }

    [StructLayout(LayoutKind.Sequential)]
    private struct EngineApi
    {
        public uint Abi;
        public uint Size;
        public nint Name;
        public nint CreateProfile;
        public nint DestroyProfile;
        public nint CreateView;
        public nint DestroyView;
        public nint Navigate;
        public nint SetVisible;
        public nint SetZoom;
        public nint Suspend;
        public nint Resume;
        public nint SetRequestPolicy;
        public nint RegisterCdm;
        public nint FreeBytes;
        public nint GoBack;
        public nint GoForward;
        public nint Reload;
        public nint Stop;
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int GetEngineApi(uint requestedAbi, out nint api);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int CreateProfile(ref ProfileConfig config, out ulong profile);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int DestroyProfile(ulong profile);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int CreateView(ulong profile, ref ViewConfig config, ref Callbacks callbacks, out ulong view);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int DestroyView(ulong view);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int NavigateView(ulong view, Bytes uri);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int SetVisible(ulong view, [MarshalAs(UnmanagedType.I1)] bool visible);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int ViewCommand(ulong view);

    public string Status { get; private set; } = "engine ABI unavailable";

    public static EngineBridge Load()
    {
        var bridge = new EngineBridge();
        string architectureName = RuntimeInformation.ProcessArchitecture == Architecture.Arm64
            ? "ubar_engine_arm64.dll"
            : "ubar_engine_x64.dll";
        if (!NativeLibrary.TryLoad(architectureName, out bridge.library)
            && !NativeLibrary.TryLoad("ubar_engine.dll", out bridge.library))
            return bridge;
        nint symbol = NativeLibrary.GetExport(bridge.library, "ubar_get_engine_api");
        var getApi = Marshal.GetDelegateForFunctionPointer<GetEngineApi>(symbol);
        int result = getApi(AbiV1, out nint apiPointer);
        if (result != 0 || apiPointer == 0) {
            bridge.Status = $"engine error {result}";
            return bridge;
        }
        bridge.api = Marshal.PtrToStructure<EngineApi>(apiPointer);
        bridge.Status = bridge.api.Abi == AbiV1
            ? Marshal.PtrToStringUTF8(bridge.api.Name) ?? "engine ABI v1"
            : "engine ABI mismatch";
        return bridge;
    }

    public void Attach(bool privateMode)
    {
        if (api.CreateProfile == 0) return;
        ulong available = (ulong)Math.Max(GC.GetGCMemoryInfo().TotalAvailableMemoryBytes, 512L * 1024 * 1024);
        var profileConfig = new ProfileConfig {
            Size = (uint)Marshal.SizeOf<ProfileConfig>(), Kind = privateMode ? 1u : 0u,
            MemoryTarget = available / 4, MemoryCeiling = available * 3 / 4,
            PartitionStorage = true, BlockThirdPartyCookies = true, RequireSandbox = true,
        };
        int result = Marshal.GetDelegateForFunctionPointer<CreateProfile>(api.CreateProfile)(ref profileConfig, out profile);
        if (result != 0) { Status = $"profile error {result}"; return; }
    }

    public ulong CreateView(nint nativeParent, uint width, uint height, double scale)
    {
        if (profile == 0 || api.CreateView == 0 || nativeParent == 0) return 0;
        var config = new ViewConfig {
            Size = (uint)Marshal.SizeOf<ViewConfig>(), NativeParent = nativeParent,
            Width = width, Height = height, DeviceScale = scale, Visible = true,
        };
        var callbacks = new Callbacks { Size = (uint)Marshal.SizeOf<Callbacks>() };
        int result = Marshal.GetDelegateForFunctionPointer<CreateView>(api.CreateView)(profile, ref config, ref callbacks, out ulong view);
        if (result != 0) Status = $"view error {result}";
        return result == 0 ? view : 0;
    }

    public void DestroyViewHandle(ulong view)
    {
        if (view != 0 && api.DestroyView != 0)
            Marshal.GetDelegateForFunctionPointer<DestroyView>(api.DestroyView)(view);
    }

    public void SetViewVisible(ulong view, bool visible)
    {
        if (view != 0 && api.SetVisible != 0)
            Marshal.GetDelegateForFunctionPointer<SetVisible>(api.SetVisible)(view, visible);
    }

    public void Navigate(ulong view, string input)
    {
        if (view == 0 || api.Navigate == 0 || string.IsNullOrWhiteSpace(input)) return;
        string uri = input.Contains("://") ? input : $"https://{input}";
        nint memory = Marshal.StringToCoTaskMemUTF8(uri);
        try {
            var bytes = new Bytes { Data = memory, Length = (nuint)System.Text.Encoding.UTF8.GetByteCount(uri) };
            Marshal.GetDelegateForFunctionPointer<NavigateView>(api.Navigate)(view, bytes);
        } finally { Marshal.FreeCoTaskMem(memory); }
    }

    private void Command(ulong view, nint function)
    {
        if (view != 0 && function != 0)
            Marshal.GetDelegateForFunctionPointer<ViewCommand>(function)(view);
    }

    public void GoBack(ulong view) => Command(view, api.GoBack);
    public void GoForward(ulong view) => Command(view, api.GoForward);
    public void Reload(ulong view) => Command(view, api.Reload);
    public void Stop(ulong view) => Command(view, api.Stop);

    public void Dispose()
    {
        if (profile != 0 && api.DestroyProfile != 0)
            Marshal.GetDelegateForFunctionPointer<DestroyProfile>(api.DestroyProfile)(profile);
        if (library != 0) NativeLibrary.Free(library);
        profile = 0;
        library = 0;
        GC.SuppressFinalize(this);
    }

    ~EngineBridge() => Dispose();
}
