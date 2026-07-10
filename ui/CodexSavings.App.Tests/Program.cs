using CodexSavings;

static void Assert(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

var controller = new FlyoutVisibilityController();
var show = controller.ToggleFromTray();
Assert(controller.State == FlyoutState.Showing, "first click shows");
var hide = controller.ToggleFromTray();
controller.Complete(show);
Assert(controller.State == FlyoutState.Hiding, "stale show cannot win a rapid second click");
controller.Complete(hide);
Assert(controller.State == FlyoutState.Hidden, "second click hides");
controller.Show();
controller.Complete(3);
Assert(controller.HideForFocusLoss(), "focus loss hides a visible flyout");
controller.BeginTrayInteraction();
Assert(!controller.HideForFocusLoss(), "tray interaction prevents focus-loss hiding");
controller.EndTrayInteraction();

Assert(SettingsValidator.IsValid(new("custom", 0, "es", 31)), "custom boundary is valid");
Assert(!SettingsValidator.IsValid(new("custom", null, "es", 31)), "custom needs an amount");
Assert(!SettingsValidator.IsValid(new("plus", null, "fr", 1)), "language is bounded");
Assert(!SettingsValidator.IsValid(new("plus", null, "en", 32)), "cycle day is bounded");

var work = new FlyoutRect(0, 0, 1920, 1040);
Assert(FlyoutPlacement.Calculate(new(1800, 1040, 1840, 1080), work, 420, 300, 12) == new FlyoutPoint(1488, 728), "bottom taskbar anchors above the work area");
Assert(FlyoutPlacement.Calculate(new(900, 0, 940, 40), new(0, 40, 1920, 1080), 420, 300, 12) == new FlyoutPoint(710, 52), "top taskbar anchors below the work area");
Assert(FlyoutPlacement.Calculate(new(0, 500, 40, 540), new(40, 0, 1920, 1080), 420, 300, 12) == new FlyoutPoint(52, 370), "left taskbar anchors right of the work area edge");
Assert(FlyoutPlacement.Calculate(new(1880, 500, 1920, 540), new(0, 0, 1880, 1080), 420, 300, 12) == new FlyoutPoint(1448, 370), "right taskbar anchors left of the work area edge");
Assert(FlyoutPlacement.AnimationOffset(new(1800, 1040, 1840, 1080), work, 10) == new FlyoutPoint(0, 10), "bottom taskbar animates from below");
Assert(FlyoutPlacement.AnimationOffset(new(900, 0, 940, 40), new(0, 40, 1920, 1080), 10) == new FlyoutPoint(0, -10), "top taskbar animates from above");
Assert(FlyoutPlacement.AnimationOffset(new(0, 500, 40, 540), new(40, 0, 1920, 1080), 10) == new FlyoutPoint(-10, 0), "left taskbar animates from the left");
Assert(FlyoutPlacement.AnimationOffset(new(1880, 500, 1920, 540), new(0, 0, 1880, 1080), 10) == new FlyoutPoint(10, 0), "right taskbar animates from the right");
Assert(FlyoutPlacement.Calculate(new(1800, 900, 1840, 940), work, new(0, 0, 1920, 1080), 420, 300, 12) == new FlyoutPoint(1488, 728), "overflow icons still follow the excluded taskbar edge");
Assert(FlyoutPlacement.Calculate(new(-120, 1040, -80, 1080), new(-1920, 0, 0, 1040), new(-1920, 0, 0, 1080), 420, 300, 12) == new FlyoutPoint(-432, 728), "negative monitor coordinates are preserved");
Assert(FlyoutPlacement.AnimationOffset(new(1800, 1000, 1840, 1020), new(0, 0, 1920, 1080), new(0, 0, 1920, 1080), 10) == new FlyoutPoint(0, 10), "auto-hide uses the nearest display edge");

var period = new PeriodDto(10, 1, 1, 1, 1, 1, 1, 1, 1);
var config = new ConfigDto("plus", null, "en", 1);
var previous = new SnapshotDto(period, period, period, config, "standard", "", "", 1, "now", "history", "", [], 0, null);
var current = previous with { AllTime = null, AllTimeUpdatedAt = "" };
var preserved = RefreshCoordinator.PreserveAllTime(current, previous);
Assert(preserved.AllTime == period && preserved.AllTimeUpdatedAt == "history", "current refresh preserves calculated history");
