using Microsoft.UI.Windowing;
using System.Runtime.InteropServices;
using Windows.Graphics;

namespace CodexSavings;

internal readonly record struct FlyoutLayout(FlyoutPoint AnimationOffset, double Scale);

internal static class FlyoutWindowNative
{
    private const int GwlStyle = -16;
    private const int GwlExStyle = -20;
    private const long WsPopup = 0x80000000;
    private const long WsCaption = 0x00c00000;
    private const long WsThickFrame = 0x00040000;
    private const long WsSysMenu = 0x00080000;
    private const long WsMinimizeBox = 0x00020000;
    private const long WsMaximizeBox = 0x00010000;
    private const long WsExToolWindow = 0x00000080;
    private const long WsExAppWindow = 0x00040000;
    private const int DwmWindowCornerPreference = 33;
    private const int DwmBorderColor = 34;
    private const int DwmWindowCornerRound = 2;
    private const uint DwmColorNone = 0xfffffffe;
    private const uint SwpNoSize = 0x0001;
    private const uint SwpNoMove = 0x0002;
    private const uint SwpNoZOrder = 0x0004;
    private const uint SwpNoActivate = 0x0010;
    private const uint SwpFrameChanged = 0x0020;
    private const int CornerRadiusEpx = 12;

    internal static void Apply(nint window)
    {
        var style = GetWindowLongPtr(window, GwlStyle).ToInt64();
        var frameStyles = WsCaption | WsThickFrame | WsSysMenu | WsMinimizeBox | WsMaximizeBox;
        SetWindowLongPtr(window, GwlStyle, new((style | WsPopup) & ~frameStyles));

        var extendedStyle = GetWindowLongPtr(window, GwlExStyle).ToInt64();
        SetWindowLongPtr(window, GwlExStyle, new((extendedStyle | WsExToolWindow) & ~WsExAppWindow));
        SetWindowPos(window, 0, 0, 0, 0, 0, SwpNoSize | SwpNoMove | SwpNoZOrder | SwpNoActivate | SwpFrameChanged);

        var corner = DwmWindowCornerRound;
        DwmSetWindowAttribute(window, DwmWindowCornerPreference, ref corner, sizeof(int));
        var border = DwmColorNone;
        DwmSetWindowAttribute(window, DwmBorderColor, ref border, sizeof(uint));

        if (GetWindowRect(window, out var bounds))
        {
            ApplyRoundedRegion(window, bounds.Right - bounds.Left, bounds.Bottom - bounds.Top, Scale(CornerRadiusEpx, GetDpi(window) / 96d));
        }
    }

    internal static uint GetDpi(nint window) => GetDpiForWindow(window) is var dpi and > 0 ? dpi : 96;

    internal static FlyoutLayout Place(
        AppWindow appWindow,
        nint window,
        FlyoutRect? iconRect,
        int widthEpx,
        int heightEpx,
        int gapEpx,
        int animationOffsetEpx)
    {
        var display = iconRect is { } icon
            ? DisplayArea.GetFromPoint(
                new PointInt32(icon.Left + (icon.Width / 2), icon.Top + (icon.Height / 2)),
                DisplayAreaFallback.Nearest)
            : DisplayArea.Primary;
        var outer = display.OuterBounds;
        var work = display.WorkArea;
        var outerArea = new FlyoutRect(outer.X, outer.Y, outer.X + outer.Width, outer.Y + outer.Height);
        var workArea = new FlyoutRect(
            outer.X + work.X,
            outer.Y + work.Y,
            outer.X + work.X + work.Width,
            outer.Y + work.Y + work.Height);
        var anchor = iconRect ?? new FlyoutRect(
            workArea.Right - 1,
            workArea.Bottom,
            workArea.Right,
            workArea.Bottom + 1);

        appWindow.Move(new PointInt32(workArea.Left + gapEpx, workArea.Top + gapEpx));
        var scale = GetDpi(window) / 96d;
        var width = Scale(widthEpx, scale);
        var height = Scale(heightEpx, scale);
        var gap = Scale(gapEpx, scale);
        var position = FlyoutPlacement.Calculate(anchor, workArea, outerArea, width, height, gap);
        appWindow.MoveAndResize(new RectInt32(position.X, position.Y, width, height));
        ApplyRoundedRegion(window, width, height, Scale(CornerRadiusEpx, scale));
        return new(
            FlyoutPlacement.AnimationOffset(anchor, workArea, outerArea, Scale(animationOffsetEpx, scale)),
            scale);
    }

    private static int Scale(int value, double scale) =>
        (int)Math.Round(value * scale, MidpointRounding.AwayFromZero);

    private static void ApplyRoundedRegion(nint window, int width, int height, int radius)
    {
        if (width <= 0 || height <= 0 || radius <= 0) return;
        var region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2);
        if (region == 0) return;
        if (SetWindowRgn(window, region, true) == 0) DeleteObject(region);
    }

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    private static extern nint GetWindowLongPtr(nint window, int index);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    private static extern nint SetWindowLongPtr(nint window, int index, nint value);

    [DllImport("user32.dll")]
    private static extern bool SetWindowPos(nint window, nint insertAfter, int x, int y, int width, int height, uint flags);

    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(nint window);

    [DllImport("user32.dll")]
    private static extern bool GetWindowRect(nint window, out NativeRect bounds);

    [DllImport("gdi32.dll")]
    private static extern nint CreateRoundRectRgn(int left, int top, int right, int bottom, int width, int height);

    [DllImport("user32.dll")]
    private static extern int SetWindowRgn(nint window, nint region, bool redraw);

    [DllImport("gdi32.dll")]
    private static extern bool DeleteObject(nint value);

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(nint window, int attribute, ref int value, int size);

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(nint window, int attribute, ref uint value, int size);

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRect
    {
        internal int Left;
        internal int Top;
        internal int Right;
        internal int Bottom;
    }
}
