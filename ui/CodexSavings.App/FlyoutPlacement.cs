namespace CodexSavings;

internal readonly record struct FlyoutRect(int Left, int Top, int Right, int Bottom)
{
    internal int Width => Right - Left;
    internal int Height => Bottom - Top;
}

internal readonly record struct FlyoutPoint(int X, int Y);

internal enum FlyoutEdge { Top, Right, Bottom, Left }

internal static class FlyoutPlacement
{
    internal static FlyoutPoint Calculate(
        FlyoutRect anchor,
        FlyoutRect workArea,
        int width,
        int height,
        int gap)
        => Calculate(anchor, workArea, workArea, width, height, gap);

    internal static FlyoutPoint Calculate(
        FlyoutRect anchor,
        FlyoutRect workArea,
        FlyoutRect outerArea,
        int width,
        int height,
        int gap)
    {
        var centerX = anchor.Left + (anchor.Width / 2);
        var centerY = anchor.Top + (anchor.Height / 2);
        var x = Clamp(centerX - (width / 2), workArea.Left + gap, workArea.Right - width - gap);
        var y = Clamp(centerY - (height / 2), workArea.Top + gap, workArea.Bottom - height - gap);

        switch (ResolveEdge(anchor, workArea, outerArea))
        {
            case FlyoutEdge.Top: y = workArea.Top + gap; break;
            case FlyoutEdge.Right: x = workArea.Right - width - gap; break;
            case FlyoutEdge.Bottom: y = workArea.Bottom - height - gap; break;
            case FlyoutEdge.Left: x = workArea.Left + gap; break;
        }

        return new(x, y);
    }

    internal static FlyoutPoint AnimationOffset(FlyoutRect anchor, FlyoutRect workArea, int distance)
        => AnimationOffset(anchor, workArea, workArea, distance);

    internal static FlyoutPoint AnimationOffset(
        FlyoutRect anchor,
        FlyoutRect workArea,
        FlyoutRect outerArea,
        int distance)
    {
        return ResolveEdge(anchor, workArea, outerArea) switch
        {
            FlyoutEdge.Top => new(0, -distance),
            FlyoutEdge.Right => new(distance, 0),
            FlyoutEdge.Bottom => new(0, distance),
            _ => new(-distance, 0),
        };
    }

    private static FlyoutEdge ResolveEdge(FlyoutRect anchor, FlyoutRect workArea, FlyoutRect outerArea)
    {
        if (anchor.Top >= workArea.Bottom) return FlyoutEdge.Bottom;
        if (anchor.Bottom <= workArea.Top) return FlyoutEdge.Top;
        if (anchor.Left >= workArea.Right) return FlyoutEdge.Right;
        if (anchor.Right <= workArea.Left) return FlyoutEdge.Left;
        if (workArea.Top > outerArea.Top) return FlyoutEdge.Top;
        if (workArea.Right < outerArea.Right) return FlyoutEdge.Right;
        if (workArea.Bottom < outerArea.Bottom) return FlyoutEdge.Bottom;
        if (workArea.Left > outerArea.Left) return FlyoutEdge.Left;

        var centerX = anchor.Left + (anchor.Width / 2);
        var centerY = anchor.Top + (anchor.Height / 2);
        var distances = new[]
        {
            (Edge: FlyoutEdge.Top, Distance: Math.Abs(centerY - workArea.Top)),
            (Edge: FlyoutEdge.Right, Distance: Math.Abs(workArea.Right - centerX)),
            (Edge: FlyoutEdge.Bottom, Distance: Math.Abs(workArea.Bottom - centerY)),
            (Edge: FlyoutEdge.Left, Distance: Math.Abs(centerX - workArea.Left)),
        };
        return distances.MinBy(item => item.Distance).Edge;
    }

    private static int Clamp(int value, int minimum, int maximum) =>
        maximum < minimum ? minimum : Math.Clamp(value, minimum, maximum);
}
