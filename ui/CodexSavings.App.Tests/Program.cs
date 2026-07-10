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
