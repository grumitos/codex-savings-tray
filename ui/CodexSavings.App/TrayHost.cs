using System.ComponentModel;
using System.Runtime.InteropServices;

namespace CodexSavings;

internal sealed class TrayHost : IDisposable
{
    private const uint WmTray = 0x8001, WmContextMenu = 0x007b, WmLButtonDown = 0x0201, WmLButtonUp = 0x0202, WmRButtonUp = 0x0205;
    private const uint NinSelect = 0x0400, NinKeySelect = 0x0401;
    private const uint NimAdd = 0, NimModify = 1, NimDelete = 2, NimSetVersion = 4, NotifyVersion4 = 4;
    private const uint NifMessage = 1, NifIcon = 2, NifTip = 4, NifShowTip = 0x80;
    private const uint WsOverlapped = 0;
    private static readonly WndProc Procedure = WindowProcedure;
    private static TrayHost? Current;
    private readonly nint _window, _icon;
    private readonly uint _taskbarCreated;
    private string _tooltip = "Codex Savings";
    private bool _disposed;
    public event Action? LeftButtonDown;
    public event Action? LeftClicked;
    public event Action? RightClicked;

    public TrayHost()
    {
        if (Current is not null) throw new InvalidOperationException("Only one tray host can be active.");
        _taskbarCreated = RegisterWindowMessage("TaskbarCreated");
        if (_taskbarCreated == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not register TaskbarCreated.");

        var name = "CodexSavings.Tray." + Environment.ProcessId;
        var instance = GetModuleHandle(null);
        if (instance == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not get the application module handle.");

        var windowClass = new WndClass { Size = Marshal.SizeOf<WndClass>(), ClassName = name, Procedure = Procedure, Instance = instance };
        if (RegisterClassEx(ref windowClass) == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not register the tray window class.");

        Current = this;
        try
        {
            // TaskbarCreated is broadcast to top-level windows, so this window is hidden rather than message-only.
            _window = CreateWindowEx(0, name, name, WsOverlapped, 0, 0, 0, 0, 0, 0, instance, 0);
            if (_window == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not create the tray window.");

            _icon = LoadImage(0, Path.Combine(AppContext.BaseDirectory, "Assets", "app.ico"), 1, 0, 0, 0x0010);
            if (_icon == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not load the tray icon.");
            if (!RestoreIcon()) throw new InvalidOperationException("Could not add the notification-area icon.");
        }
        catch
        {
            Dispose();
            throw;
        }
    }

    private bool SetVersion()
    {
        var version = CreateIconData(_tooltip);
        version.Version = NotifyVersion4;
        return ShellNotifyIcon(NimSetVersion, ref version);
    }

    private bool RestoreIcon() => Update(NimAdd, _tooltip) && SetVersion();

    public void SetTooltip(string text)
    {
        ArgumentNullException.ThrowIfNull(text);
        _tooltip = LimitTooltip(text);
        if (!Update(NimModify, _tooltip)) RestoreIcon();
    }

    private bool Update(uint action, string tip)
    {
        var icon = CreateIconData(tip);
        return ShellNotifyIcon(action, ref icon);
    }

    private static string LimitTooltip(string text) => text[..Math.Min(text.Length, 127)];

    private NotifyIconData CreateIconData(string tip) => new()
    {
        Size = Marshal.SizeOf<NotifyIconData>(),
        Window = _window,
        Id = 1,
        Flags = NifMessage | NifIcon | NifTip | NifShowTip,
        CallbackMessage = WmTray,
        Icon = _icon,
        Tip = LimitTooltip(tip),
    };
    public FlyoutRect? GetIconRect()
    {
        var identifier = new NotifyIconIdentifier
        {
            Size = Marshal.SizeOf<NotifyIconIdentifier>(),
            Window = _window,
            Id = 1,
        };
        return ShellNotifyIconGetRect(ref identifier, out var rect) >= 0
            ? new(rect.Left, rect.Top, rect.Right, rect.Bottom)
            : null;
    }
    private static nint WindowProcedure(nint window, uint message, nint wParam, nint lParam)
    {
        if (Current is not null && message == Current._taskbarCreated)
        {
            Current.RestoreIcon();
            return 0;
        }
        if (message == WmTray && Current is not null)
        {
            var notification = (uint)(lParam.ToInt64() & 0xffff);
            var iconId = (uint)((lParam.ToInt64() >> 16) & 0xffff);
            if (iconId != 1) return DefWindowProc(window, message, wParam, lParam);

            if (notification == WmLButtonDown) Current.LeftButtonDown?.Invoke();
            if (notification is WmLButtonUp or NinSelect or NinKeySelect) Current.LeftClicked?.Invoke();
            if (notification is WmRButtonUp or WmContextMenu) Current.RightClicked?.Invoke();
            return 0;
        }
        return DefWindowProc(window, message, wParam, lParam);
    }
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        if (ReferenceEquals(Current, this)) Current = null;
        if (_window != 0) Update(NimDelete, string.Empty);
        if (_icon != 0) DestroyIcon(_icon);
        if (_window != 0) DestroyWindow(_window);
    }
    private delegate nint WndProc(nint window, uint message, nint wParam, nint lParam);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct WndClass { public int Size; public uint Style; public WndProc Procedure; public int Extra; public int WindowExtra; public nint Instance; public nint Icon; public nint Cursor; public nint Background; public string? MenuName; public string ClassName; public nint SmallIcon; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NotifyIconData { public int Size; public nint Window; public uint Id; public uint Flags; public uint CallbackMessage; public nint Icon; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string Tip; public uint State; public uint StateMask; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string? Info; public uint Version; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string? InfoTitle; public uint InfoFlags; public Guid Guid; public nint BalloonIcon; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NotifyIconIdentifier { public int Size; public nint Window; public uint Id; public Guid Guid; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRect { public int Left; public int Top; public int Right; public int Bottom; }
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern ushort RegisterClassEx(ref WndClass windowClass);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern nint CreateWindowEx(uint extendedStyle, string className, string windowName, uint style, int x, int y, int width, int height, nint parent, nint menu, nint instance, nint parameter);
    [DllImport("user32.dll")] private static extern nint DefWindowProc(nint window, uint message, nint wParam, nint lParam);
    [DllImport("user32.dll")] private static extern bool DestroyWindow(nint window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern nint LoadImage(nint instance, string name, uint type, int cx, int cy, uint flags);
    [DllImport("user32.dll")] private static extern bool DestroyIcon(nint icon);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern uint RegisterWindowMessage(string message);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern nint GetModuleHandle(string? moduleName);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)] private static extern bool ShellNotifyIcon(uint message, ref NotifyIconData data);
    [DllImport("shell32.dll")] private static extern int ShellNotifyIconGetRect(ref NotifyIconIdentifier identifier, out NativeRect iconLocation);
}
