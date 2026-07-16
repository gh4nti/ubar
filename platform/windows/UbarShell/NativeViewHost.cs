using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace UbarShell;

internal sealed class NativeViewHost : Grid, IDisposable
{
    private const uint WS_CHILD = 0x40000000;
    private const uint WS_VISIBLE = 0x10000000;
    private const uint WS_CLIPSIBLINGS = 0x04000000;
    private const uint WS_CLIPCHILDREN = 0x02000000;
    private readonly nint parent;
    public nint Handle { get; private set; }
    public event Action<NativeViewHost>? HandleReady;

    public NativeViewHost(Window window)
    {
        parent = WinRT.Interop.WindowNative.GetWindowHandle(window);
        Loaded += OnLoaded;
        SizeChanged += (_, _) => Place();
        LayoutUpdated += (_, _) => Place();
    }

    private void OnLoaded(object sender, RoutedEventArgs args)
    {
        if (Handle != 0) return;
        Handle = CreateWindowEx(0, "STATIC", null,
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
            0, 0, 1, 1, parent, 0, 0, 0);
        if (Handle == 0) throw new InvalidOperationException("Could not create renderer host HWND");
        Place();
        HandleReady?.Invoke(this);
    }

    private void Place()
    {
        if (Handle == 0 || XamlRoot == null) return;
        var point = TransformToVisual(null).TransformPoint(new Windows.Foundation.Point());
        double scale = XamlRoot.RasterizationScale;
        SetWindowPos(Handle, 0,
            (int)Math.Round(point.X * scale), (int)Math.Round(point.Y * scale),
            Math.Max(1, (int)Math.Round(ActualWidth * scale)),
            Math.Max(1, (int)Math.Round(ActualHeight * scale)),
            0x0010 | 0x0004);
    }

    public void SetNativeVisible(bool visible)
    {
        if (Handle != 0) ShowWindow(Handle, visible ? 5 : 0);
    }

    public void Dispose()
    {
        if (Handle != 0) DestroyWindow(Handle);
        Handle = 0;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern nint CreateWindowEx(uint exStyle, string className, string? windowName,
        uint style, int x, int y, int width, int height, nint parent, nint menu, nint instance, nint param);
    [DllImport("user32.dll")]
    private static extern bool DestroyWindow(nint window);
    [DllImport("user32.dll")]
    private static extern bool ShowWindow(nint window, int command);
    [DllImport("user32.dll")]
    private static extern bool SetWindowPos(nint window, nint after, int x, int y, int width, int height, uint flags);
}
