using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI;
using Microsoft.Windows.AppLifecycle;
using System.Diagnostics;
using Windows.Graphics;

namespace CodexSavings;

public partial class DesktopApp : Application
{
    private readonly CoreBridge _core = new();
    private TrayHost? _tray;
    private RefreshCoordinator? _refresh;
    private SummaryWindow? _summary;
    private SettingsWindow? _settings;
    public DesktopApp()
    {
        InitializeComponent();
        AppDomain.CurrentDomain.ProcessExit += (_, _) => DisposeInfrastructure();
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        if (Environment.GetCommandLineArgs().Contains("--show"))
        {
            _refresh = new RefreshCoordinator(_core);
            _summary = new SummaryWindow(_refresh);
            _summary.Present();
            return;
        }
        var dispatcher = DispatcherQueue.GetForCurrentThread();
        var instance = AppInstance.FindOrRegisterForKey("CodexSavings.App");
        if (!instance.IsCurrent) { await instance.RedirectActivationToAsync(AppInstance.GetCurrent().GetActivatedEventArgs()); Exit(); return; }
        instance.Activated += (_, _) => dispatcher.TryEnqueue(ShowSummary);
        _refresh = new RefreshCoordinator(_core);
        _refresh.SnapshotChanged += snapshot => dispatcher.TryEnqueue(() =>
        {
            _summary?.Update(snapshot);
            _tray?.SetTooltip(snapshot.Config.Plan + ": $" + snapshot.Cycle.CostUsd.ToString("F2"));
        });
        _refresh.Failed += error => dispatcher.TryEnqueue(() => _summary?.ShowError(error));
        _tray = new TrayHost();
        _tray.LeftClicked += () => dispatcher.TryEnqueue(ToggleSummary);
        _tray.RightClicked += () => dispatcher.TryEnqueue(ShowQuickActions);
        await _refresh.StartAsync();
    }
    private void ToggleSummary() { if (_summary is not null) { _summary.Close(); _summary = null; } else ShowSummary(); }
    private void ShowSummary() { _summary ??= new SummaryWindow(_refresh!); _summary.Present(); }
    private void ShowQuickActions()
    {
        _summary?.Close(); _summary = null;
        new QuickActionsWindow(ToggleSummary, () => _ = _refresh!.RefreshAsync(), ShowSettings,
            () => Process.Start(new ProcessStartInfo("https://chatgpt.com/codex/cloud/settings/analytics") { UseShellExecute = true }), Shutdown).Activate();
    }
    private void ShowSettings()
    {
        _summary?.Close(); _summary = null;
        if (_settings is not null) { _settings.Activate(); return; }
        _settings = new SettingsWindow(_core, () => { _settings = null; _ = _refresh!.RefreshAsync(); });
        _settings.Activate();
    }
    private void Shutdown()
    {
        DisposeInfrastructure();
        Exit();
    }
    private void DisposeInfrastructure()
    {
        _tray?.Dispose();
        _tray = null;
        if (_refresh is not null) _ = _refresh.DisposeAsync();
        _refresh = null;
    }
}

internal sealed class SummaryWindow : Window
{
    private readonly RefreshCoordinator _refresh;
    private readonly TextBlock _plan = new();
    private readonly TextBlock _tier = new();
    private readonly TextBlock _cost = new() { FontSize = 32, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly ProgressBar _progress = new() { Minimum = 0, Maximum = 100, Height = 6 };
    private readonly TextBlock _context = new() { Opacity = .72 };
    private readonly TextBlock _today = new() { FontSize = 17, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly TextBlock _days = new() { FontSize = 17, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly TextBlock _allTime = new() { FontSize = 17, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly TextBlock _updated = new() { Opacity = .65, VerticalAlignment = VerticalAlignment.Center };
    private readonly InfoBar _error = new() { IsOpen = false, Severity = InfoBarSeverity.Error };
    private readonly ProgressRing _busy = new() { IsActive = false, Width = 20, Height = 20 };
    public SummaryWindow(RefreshCoordinator refresh)
    {
        _refresh = refresh; Title = "Codex Savings"; SystemBackdrop = new DesktopAcrylicBackdrop();
        var stats = new Grid { ColumnSpacing = 18 };
        stats.ColumnDefinitions.Add(new ColumnDefinition());
        stats.ColumnDefinitions.Add(new ColumnDefinition());
        stats.ColumnDefinitions.Add(new ColumnDefinition());
        stats.Children.Add(Stat("Today", _today, 0));
        stats.Children.Add(Stat("Until reset", _days, 1));
        stats.Children.Add(Stat("All time", _allTime, 2));
        Content = new StackPanel { Spacing = 9, Padding = new Thickness(20, 16, 20, 14), Width = 420, Height = 300, Children =
        {
            new Grid { Children =
            {
                new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children =
                {
                    new TextBlock { Text = "Codex Savings", FontSize = 18, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold },
                    Chip(_plan), Chip(_tier),
                }},
            }},
            _cost, _progress, _context, stats, _error,
            new Grid { Children =
            {
                _updated,
                new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Right, Children =
                {
                    _busy,
                    new Button { Content = "Refresh" }.Also(button => button.Click += async (_, _) => await RefreshAsync()),
                }},
            }},
        }};
        if (_refresh.LastSnapshot is not null) Update(_refresh.LastSnapshot); else _ = RefreshAsync();
    }
    public void Update(SnapshotDto snapshot)
    {
        var planUsd = snapshot.Config.MonthlyUsdOverride ?? snapshot.Config.Plan switch
        {
            "go" => 8, "plus" => 20, "pro_5x" => 100, "pro_20x" => 200, "business" => 20, _ => 0,
        };
        _plan.Text = snapshot.Config.Plan.Replace('_', ' ');
        _tier.Text = snapshot.ServiceTier;
        _cost.Text = "$" + snapshot.Cycle.CostUsd.ToString("F2");
        _progress.Visibility = planUsd > 0 ? Visibility.Visible : Visibility.Collapsed;
        _progress.Value = planUsd > 0 ? Math.Min(100, snapshot.Cycle.CostUsd / planUsd * 100) : 0;
        _context.Text = planUsd <= 0 ? "API-equivalent value this cycle" :
            snapshot.Cycle.CostUsd >= planUsd
                ? "$" + (snapshot.Cycle.CostUsd - planUsd).ToString("F2") + " saved beyond the plan price"
                : "$" + (planUsd - snapshot.Cycle.CostUsd).ToString("F2") + " left to break even";
        _today.Text = "$" + snapshot.Today.CostUsd.ToString("F2");
        _days.Text = snapshot.DaysUntilReset.ToString();
        _allTime.Text = snapshot.AllTime is null ? "Calculate" : "$" + snapshot.AllTime.CostUsd.ToString("F2");
        _updated.Text = "Updated " + snapshot.UpdatedAt;
    }
    private static Border Chip(TextBlock text) => new()
    {
        CornerRadius = new CornerRadius(12),
        Padding = new Thickness(8, 3, 8, 3),
        Background = new SolidColorBrush(Windows.UI.Color.FromArgb(28, 255, 255, 255)),
        Child = text,
    };
    private static StackPanel Stat(string label, TextBlock value, int column)
    {
        var panel = new StackPanel { Spacing = 2, Children = { value, new TextBlock { Text = label, Opacity = .62 } } };
        Grid.SetColumn(panel, column);
        return panel;
    }
    public void Present()
    {
        Activate();
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(WinRT.Interop.WindowNative.GetWindowHandle(this)));
        appWindow.Resize(new SizeInt32(420, 300));
        if (appWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
            presenter.SetBorderAndTitleBar(false, false);
        }
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
