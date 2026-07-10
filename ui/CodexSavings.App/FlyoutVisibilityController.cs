namespace CodexSavings;

internal enum FlyoutState { Hidden, Showing, Visible, Hiding }

internal sealed class FlyoutVisibilityController
{
    private int _generation;
    public FlyoutState State { get; private set; } = FlyoutState.Hidden;
    public bool TrayInteraction { get; private set; }

    public void BeginTrayInteraction() => TrayInteraction = true;
    public void EndTrayInteraction() => TrayInteraction = false;
    public int ToggleFromTray() => State is FlyoutState.Hidden or FlyoutState.Hiding ? Show() : Hide();
    public int Show() { State = FlyoutState.Showing; return ++_generation; }
    public int Hide() { State = FlyoutState.Hiding; return ++_generation; }
    public void Complete(int generation)
    {
        if (generation != _generation) return;
        State = State == FlyoutState.Showing ? FlyoutState.Visible : FlyoutState.Hidden;
    }
    public bool HideForFocusLoss() => !TrayInteraction && State == FlyoutState.Visible;
}
