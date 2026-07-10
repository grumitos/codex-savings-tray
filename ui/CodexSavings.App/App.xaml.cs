using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.Windows.AppLifecycle;
using System.Diagnostics;

namespace CodexSavings;

public sealed partial class App : Application
{
    private readonly CoreBridge _core = new();
    private TrayHost? _tray;
    private RefreshCoordinator? _refresh;
    private SummaryWindow? _summary;
    private SettingsWindow? _settings;
    public App() => InitializeComponent();

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        var instance = AppInstance.FindOrRegisterForKey("CodexSavings.App");
        if (!instance.IsCurrent) { await instance.RedirectActivationToAsync(AppInstance.GetCurrent().GetActivatedEventArgs()); Exit(); return; }
        instance.Activated += (_, _) => DispatcherQueue.TryEnqueue(ShowSummary);
        _refresh = new RefreshCoordinator(_core);
        _refresh.SnapshotChanged += snapshot => DispatcherQueue.TryEnqueue(() =>
        {
            _summary?.Update(snapshot);
            _tray?.SetTooltip(snapshot.Config.Plan + ": $" + snapshot.Cycle.CostUsd.ToString("F2"));
        });
        _refresh.Failed += error => DispatcherQueue.TryEnqueue(() => _summary?.ShowError(error));
        _tray = new TrayHost();
        _tray.LeftClicked += () => DispatcherQueue.TryEnqueue(ToggleSummary);
        _tray.RightClicked += () => DispatcherQueue.TryEnqueue(ShowQuickActions);
        await _refresh.StartAsync();
    }
    private void ToggleSummary() { if (_summary is not null) { _summary.Close(); _summary = null; } else ShowSummary(); }
    private void ShowSummary() { _summary ??= new SummaryWindow(_refresh!); _summary.Activate(); }
    private void ShowQuickActions()
    {
        _summary?.Close(); _summary = null;
        new QuickActionsWindow(ToggleSummary, () => _ = _refresh!.RefreshAsync(), ShowSettings,
            () => Process.Start(new ProcessStartInfo("https://chatgpt.com/codex/cloud/settings/analytics") { UseShellExecute = true }), Exit).Activate();
    }
    private void ShowSettings()
    {
        _summary?.Close(); _summary = null;
        if (_settings is not null) { _settings.Activate(); return; }
        _settings = new SettingsWindow(_core, () => { _settings = null; _ = _refresh!.RefreshAsync(); });
        _settings.Activate();
    }
    protected override void OnExiting(object sender, object e) { _tray?.Dispose(); if (_refresh is not null) _ = _refresh.DisposeAsync(); base.OnExiting(sender, e); }
}

internal sealed class SummaryWindow : Window
{
    private readonly RefreshCoordinator _refresh;
    private readonly TextBlock _cost = new() { FontSize = 32 };
    private readonly TextBlock _stats = new();
    private readonly InfoBar _error = new() { IsOpen = false, Severity = InfoBarSeverity.Error };
    private readonly ProgressRing _busy = new() { IsActive = false, Width = 20, Height = 20 };
    public SummaryWindow(RefreshCoordinator refresh)
    {
        _refresh = refresh; SystemBackdrop = new DesktopAcrylicBackdrop();
        Content = new StackPanel { Spacing = 12, Padding = new Thickness(20), Children =
        {
            new TextBlock { Text = "Codex Savings", FontSize = 18 }, _cost, _stats, _error,
            new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children =
            {
                new Button { Content = "Refresh" }.Also(button => button.Click += async (_, _) => await RefreshAsync()), _busy,
            }},
        }};
        if (_refresh.LastSnapshot is not null) Update(_refresh.LastSnapshot); else _ = RefreshAsync();
    }
    public void Update(SnapshotDto snapshot)
    {
        _cost.Text = "Cycle API value: $" + snapshot.Cycle.CostUsd.ToString("F2");
        _stats.Text = "Today $" + snapshot.Today.CostUsd.ToString("F2") + "  •  " + snapshot.DaysUntilReset + " days left  •  Total " + (snapshot.AllTime is null ? "Calculate" : "$" + snapshot.AllTime.CostUsd.ToString("F2"));
    }
    public void ShowError(CoreError error) { _error.Title = error.Code; _error.Message = error.Message; _error.IsOpen = true; }
    private async Task RefreshAsync() { _busy.IsActive = true; await _refresh.RefreshAsync(); _busy.IsActive = false; }
}

internal sealed class QuickActionsWindow : Window
{
    public QuickActionsWindow(Action summary, Action refresh, Action settings, Action usage, Action exit)
    {
        SystemBackdrop = new DesktopAcrylicBackdrop();
        Content = new StackPanel { Padding = new Thickness(16), Spacing = 6, Children =
        { Button("Show / hide summary", summary), Button("Refresh", refresh), Button("Settings", settings), Button("Open usage dashboard", usage), Button("Exit", exit) }};
    }
    private static Button Button(string text, Action action) { var button = new Button { Content = text }; button.Click += (_, _) => action(); return button; }
}

internal sealed class SettingsWindow : Window
{
    private readonly CoreBridge _core; private readonly Action _saved;
    private readonly ComboBox _plans = new(); private readonly NumberBox _amount = new() { Minimum = 0, Maximum = 1_000_000 };
    private readonly NumberBox _day = new() { Minimum = 1, Maximum = 31 }; private readonly ComboBox _language = new() { ItemsSource = new[] { "auto", "en", "es" } };
    private ConfigDto? _initial;
    public SettingsWindow(CoreBridge core, Action saved)
    {
        _core = core; _saved = saved; SystemBackdrop = new MicaBackdrop(); Title = "Codex Savings settings";
        Content = new StackPanel { Padding = new Thickness(24), Spacing = 12, Children =
        {
            new TextBlock { Text = "Subscription" }, _plans, new TextBlock { Text = "Custom monthly amount (USD)" }, _amount,
            new TextBlock { Text = "Cycle day (1–31)" }, _day, new TextBlock { Text = "Language" }, _language,
            new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children =
            { new Button { Content = "Save" }.Also(button => button.Click += async (_, _) => await SaveAsync()), new Button { Content = "Cancel" }.Also(button => button.Click += (_, _) => Close()) }},
        }};
        _ = LoadAsync();
    }
    private async Task LoadAsync()
    {
        var reply = await _core.LoadSettingsAsync(); if (!reply.Ok || reply.Data is null) return;
        _initial = reply.Data.Config; _plans.ItemsSource = reply.Data.Plans; _plans.DisplayMemberPath = nameof(PlanDto.NameEn);
        _plans.SelectedItem = reply.Data.Plans.First(plan => plan.Id == _initial.Plan); _amount.Value = _initial.MonthlyUsdOverride ?? 0; _day.Value = _initial.CycleDay; _language.SelectedItem = _initial.Language;
    }
    private async Task SaveAsync()
    {
        if (_plans.SelectedItem is not PlanDto plan || _initial is null) return;
        var config = new ConfigDto(plan.Id, plan.Id == "custom" ? _amount.Value : null, _language.SelectedItem?.ToString() ?? "auto", (ushort)_day.Value);
        if (!SettingsValidator.IsValid(config)) return;
        var reply = await _core.SaveSettingsAsync(config); if (reply.Ok) { _saved(); Close(); }
    }
}
internal static class ControlExtensions { internal static T Also<T>(this T control, Action<T> configure) where T : Control { configure(control); return control; } }
