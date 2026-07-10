using System.Runtime.InteropServices;

namespace CodexSavings;

internal sealed class TrayHost : IDisposable
{
    private const uint WmTray = 0x8001, WmLButtonUp = 0x0202, WmRButtonUp = 0x0205;
    private const uint NimAdd = 0, NimModify = 1, NimDelete = 2, NifMessage = 1, NifIcon = 2, NifTip = 4;
    private static readonly WndProc Procedure = WindowProcedure;
    private static TrayHost? Current;
    private readonly nint _window, _icon;
    public event Action? LeftClicked;
    public event Action? RightClicked;

    public TrayHost()
    {
        Current = this;
        var name = "CodexSavings.Tray." + Environment.ProcessId;
        var windowClass = new WndClass { Size = Marshal.SizeOf<WndClass>(), ClassName = name, Procedure = Procedure };
        RegisterClassEx(ref windowClass);
        _window = CreateWindowEx(0, name, name, 0, 0, 0, 0, 0, new nint(-3), 0, 0, 0);
        _icon = LoadImage(0, Path.Combine(AppContext.BaseDirectory, "Assets", "app.ico"), 1, 0, 0, 0x0010);
        Update(NimAdd, "Codex Savings");
    }
    public void SetTooltip(string text) => Update(NimModify, text);
    private void Update(uint action, string tip)
    {
        var icon = new NotifyIconData { Size = Marshal.SizeOf<NotifyIconData>(), Window = _window, Id = 1, Flags = NifMessage | NifIcon | NifTip, CallbackMessage = WmTray, Icon = _icon, Tip = tip[..Math.Min(tip.Length, 127)] };
        ShellNotifyIcon(action, ref icon);
    }
    private static nint WindowProcedure(nint window, uint message, nint wParam, nint lParam)
    {
        if (message == WmTray)
        {
            if ((uint)lParam == WmLButtonUp) Current?.LeftClicked?.Invoke();
            if ((uint)lParam == WmRButtonUp) Current?.RightClicked?.Invoke();
        }
        return DefWindowProc(window, message, wParam, lParam);
    }
    public void Dispose()
    {
        Update(NimDelete, string.Empty);
        if (_icon != 0) DestroyIcon(_icon);
        if (_window != 0) DestroyWindow(_window);
        Current = null;
    }
    private delegate nint WndProc(nint window, uint message, nint wParam, nint lParam);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct WndClass { public int Size; public uint Style; public WndProc Procedure; public int Extra; public int WindowExtra; public nint Instance; public nint Icon; public nint Cursor; public nint Background; public string? MenuName; public string ClassName; public nint SmallIcon; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NotifyIconData { public int Size; public nint Window; public uint Id; public uint Flags; public uint CallbackMessage; public nint Icon; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string Tip; public uint State; public uint StateMask; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string? Info; public uint Version; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string? InfoTitle; public uint InfoFlags; public Guid Guid; public nint BalloonIcon; }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern ushort RegisterClassEx(ref WndClass windowClass);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern nint CreateWindowEx(uint extendedStyle, string className, string windowName, uint style, int x, int y, int width, int height, nint parent, nint menu, nint instance, nint parameter);
    [DllImport("user32.dll")] private static extern nint DefWindowProc(nint window, uint message, nint wParam, nint lParam);
    [DllImport("user32.dll")] private static extern bool DestroyWindow(nint window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern nint LoadImage(nint instance, string name, uint type, int cx, int cy, uint flags);
    [DllImport("user32.dll")] private static extern bool DestroyIcon(nint icon);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)] private static extern bool ShellNotifyIcon(uint message, ref NotifyIconData data);
}
