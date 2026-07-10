using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI;
using Microsoft.Windows.AppLifecycle;
using System.Diagnostics;
using Windows.Graphics;
using Windows.UI.ViewManagement;

namespace CodexSavings;

public partial class DesktopApp : Application
{
    private readonly CoreBridge _core = new();
    private readonly FlyoutVisibilityController _visibility = new();
    private readonly UISettings _uiSettings = new();
    private TrayHost? _tray;
    private RefreshCoordinator? _refresh;
    private SummaryWindow? _summary;
    private QuickActionsWindow? _quickActions;
    private SettingsWindow? _settings;
    private bool _disposed;

    public DesktopApp()
    {
        InitializeComponent();
        AppDomain.CurrentDomain.ProcessExit += (_, _) => DisposeInfrastructure();
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        var showAfterLaunch = Environment.GetCommandLineArgs().Contains("--show");
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
        _tray.LeftButtonDown += _visibility.BeginTrayInteraction;
        _tray.LeftClicked += ToggleSummaryFromTray;
        _tray.RightClicked += ShowQuickActions;
        await _refresh.StartAsync();
        if (showAfterLaunch) ShowSummary();
    }

    private void ToggleSummaryFromTray()
    {
        var generation = _visibility.ToggleFromTray();
        var show = _visibility.State == FlyoutState.Showing;
        var transition = TransitionSummaryAsync(generation, show);
        _visibility.EndTrayInteraction();
        _ = transition;
    }

    private void ToggleSummary()
    {
        var generation = _visibility.ToggleFromTray();
        _ = TransitionSummaryAsync(generation, _visibility.State == FlyoutState.Showing);
    }

    private void ShowSummary()
    {
        _quickActions?.Close();
        var generation = _visibility.Show();
        _ = TransitionSummaryAsync(generation, true);
    }

    private async Task TransitionSummaryAsync(int generation, bool show)
    {
        if (_disposed) return;
        _summary ??= new SummaryWindow(_refresh!, OnSummaryDeactivated, ShowSettings);
        var completed = show
            ? await _summary.ShowAsync(_tray?.GetIconRect(), _uiSettings.AnimationsEnabled)
            : await _summary.HideAsync(_uiSettings.AnimationsEnabled);
        if (completed) _visibility.Complete(generation);
    }

    private void OnSummaryDeactivated()
    {
        if (!_visibility.HideForFocusLoss()) return;
        var generation = _visibility.Hide();
        _ = TransitionSummaryAsync(generation, false);
    }

    private async void ShowQuickActions()
    {
        await HideSummaryAsync();
        _quickActions?.Close();
        QuickActionsWindow? quickActions = null;
        quickActions = new QuickActionsWindow(
            ToggleSummary,
            () => _ = _refresh!.RefreshAsync(),
            () => _ = _refresh!.CalculateAllTimeAsync(),
            ShowSettings,
            () => Process.Start(new ProcessStartInfo("https://chatgpt.com/codex/cloud/settings/analytics") { UseShellExecute = true }),
            Shutdown,
            () => { if (ReferenceEquals(_quickActions, quickActions)) _quickActions = null; });
        _quickActions = quickActions;
        quickActions.Present(_tray?.GetIconRect());
    }

    private async void ShowSettings()
    {
        await HideSummaryAsync();
        if (_settings is not null) { _settings.Present(); return; }
        var settings = new SettingsWindow(_core, () => _ = _refresh!.RefreshAsync());
        settings.Closed += (_, _) => { if (ReferenceEquals(_settings, settings)) _settings = null; };
        _settings = settings;
        settings.Present();
    }

    private async Task HideSummaryAsync()
    {
        if (_visibility.State == FlyoutState.Hidden) return;
        var generation = _visibility.Hide();
        await TransitionSummaryAsync(generation, false);
    }

    private void Shutdown()
    {
        DisposeInfrastructure();
        Exit();
    }
    private void DisposeInfrastructure()
    {
        if (_disposed) return;
        _disposed = true;
        _summary?.Close();
        _summary = null;
        _quickActions?.Close();
        _quickActions = null;
        _settings?.Close();
        _settings = null;
        _tray?.Dispose();
        _tray = null;
        if (_refresh is not null) _ = _refresh.DisposeAsync();
        _refresh = null;
    }
}

internal sealed class SummaryWindow : Window
{
    private const int WidthEpx = 420;
    private const int HeightEpx = 300;
    private const int GapEpx = 12;
    private const int AnimationOffsetEpx = 10;
    private readonly RefreshCoordinator _refresh;
    private readonly Action _deactivated;
    private readonly Border _root = new() { Width = WidthEpx, Height = HeightEpx, CornerRadius = new CornerRadius(12) };
    private readonly TranslateTransform _translation = new();
    private readonly TextBlock _plan = new();
    private readonly TextBlock _tier = new();
    private readonly TextBlock _cost = new() { FontSize = 32, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly ProgressBar _progress = new() { Minimum = 0, Maximum = 100, Height = 6 };
    private readonly TextBlock _context = new() { Opacity = .72 };
    private readonly TextBlock _today = new() { FontSize = 17, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly TextBlock _days = new() { FontSize = 17, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly Button _allTime = new()
    {
        FontSize = 17,
        FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        Padding = new Thickness(0),
        MinWidth = 0,
        MinHeight = 0,
        Background = new SolidColorBrush(Windows.UI.Color.FromArgb(0, 0, 0, 0)),
        BorderThickness = new Thickness(0),
        HorizontalAlignment = HorizontalAlignment.Left,
    };
    private readonly TextBlock _updated = new() { Opacity = .65, VerticalAlignment = VerticalAlignment.Center };
    private readonly InfoBar _error = new() { IsOpen = false, Severity = InfoBarSeverity.Error };
    private readonly ProgressRing _busy = new() { IsActive = false, Width = 20, Height = 20 };
    private AppWindow? _appWindow;
    private OverlappedPresenter? _presenter;
    private Storyboard? _animation;
    private TaskCompletionSource<bool>? _animationCompletion;
    private nint _windowHandle;
    private FlyoutPoint _hideOffset;
    private bool _isShown;

    public SummaryWindow(RefreshCoordinator refresh, Action deactivated, Action openSettings)
    {
        _refresh = refresh;
        _deactivated = deactivated;
        Title = "Codex Savings";
        SystemBackdrop = new DesktopAcrylicBackdrop();
        ExtendsContentIntoTitleBar = true;
        Activated += (_, eventArgs) =>
        {
            if (eventArgs.WindowActivationState == WindowActivationState.Deactivated) _deactivated();
        };
        var stats = new Grid { ColumnSpacing = 18 };
        stats.ColumnDefinitions.Add(new ColumnDefinition());
        stats.ColumnDefinitions.Add(new ColumnDefinition());
        stats.ColumnDefinitions.Add(new ColumnDefinition());
        stats.Children.Add(Stat("Today", _today, 0));
        stats.Children.Add(Stat("Until reset", _days, 1));
        stats.Children.Add(Stat("All time", _allTime, 2));
        var settingsButton = new Button
        {
            Content = new FontIcon { Glyph = "\uE713", FontSize = 15 },
            Width = 32,
            Height = 32,
            Padding = new Thickness(0),
            HorizontalAlignment = HorizontalAlignment.Right,
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(settingsButton, "Settings");
        ToolTipService.SetToolTip(settingsButton, "Settings");
        settingsButton.Click += (_, _) => openSettings();
        _allTime.Click += async (_, _) =>
        {
            if (_refresh.LastSnapshot?.AllTime is null) await CalculateAllTimeAsync();
        };
        var header = new Grid();
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        header.ColumnDefinitions.Add(new ColumnDefinition());
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        header.Children.Add(new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, VerticalAlignment = VerticalAlignment.Center, Children =
        {
            new Image { Source = new BitmapImage(new Uri("ms-appx:///Assets/app.png")), Width = 22, Height = 22 },
            new TextBlock { Text = "Codex Savings", FontSize = 18, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, VerticalAlignment = VerticalAlignment.Center },
            Chip(_plan), Chip(_tier),
        }});
        Grid.SetColumn(settingsButton, 2);
        header.Children.Add(settingsButton);
        _root.RenderTransform = _translation;
        _root.Child = new StackPanel { Spacing = 9, Padding = new Thickness(20, 16, 20, 14), Children =
        {
            header,
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
        Content = _root;
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
        _allTime.Content = snapshot.AllTime is null ? "Calculate" : "$" + snapshot.AllTime.CostUsd.ToString("F2");
        _updated.Text = "Updated " + snapshot.UpdatedAt;
    }
    private static Border Chip(TextBlock text) => new()
    {
        CornerRadius = new CornerRadius(12),
        Padding = new Thickness(8, 3, 8, 3),
        Background = new SolidColorBrush(Windows.UI.Color.FromArgb(28, 255, 255, 255)),
        Child = text,
    };
    private static StackPanel Stat(string label, FrameworkElement value, int column)
    {
        var panel = new StackPanel { Spacing = 2, Children = { value, new TextBlock { Text = label, Opacity = .62 } } };
        Grid.SetColumn(panel, column);
        return panel;
    }
    public async Task<bool> ShowAsync(FlyoutRect? iconRect, bool animationsEnabled)
    {
        CancelAnimationPreservingValues();
        var appWindow = EnsureAppWindow();
        var layout = FlyoutWindowNative.Place(
            appWindow,
            _windowHandle,
            iconRect,
            WidthEpx,
            HeightEpx,
            GapEpx,
            AnimationOffsetEpx);
        var scale = layout.Scale;
        _hideOffset = layout.AnimationOffset;

        if (!_isShown)
        {
            _root.Opacity = animationsEnabled ? 0 : 1;
            _translation.X = animationsEnabled ? _hideOffset.X / scale : 0;
            _translation.Y = animationsEnabled ? _hideOffset.Y / scale : 0;
        }
        _presenter!.IsAlwaysOnTop = true;
        _isShown = true;
        Activate();

        if (!animationsEnabled)
        {
            _root.Opacity = 1;
            _translation.X = 0;
            _translation.Y = 0;
            return true;
        }
        return await AnimateAsync(1, 0, 0, 160, EasingMode.EaseOut);
    }

    public async Task<bool> HideAsync(bool animationsEnabled)
    {
        CancelAnimationPreservingValues();
        if (!_isShown || _appWindow is null) return true;
        var scale = FlyoutWindowNative.GetDpi(_windowHandle) / 96d;
        var completed = !animationsEnabled ||
            await AnimateAsync(0, _hideOffset.X / scale, _hideOffset.Y / scale, 120, EasingMode.EaseIn);
        if (!completed) return false;
        _presenter!.IsAlwaysOnTop = false;
        _appWindow.Hide();
        _isShown = false;
        return true;
    }

    private AppWindow EnsureAppWindow()
    {
        if (_appWindow is not null) return _appWindow;
        _windowHandle = WinRT.Interop.WindowNative.GetWindowHandle(this);
        _appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(_windowHandle));
        _appWindow.IsShownInSwitchers = false;
        _presenter = (OverlappedPresenter)_appWindow.Presenter;
        _presenter.IsResizable = false;
        _presenter.IsMaximizable = false;
        _presenter.IsMinimizable = false;
        _presenter.SetBorderAndTitleBar(false, false);
        FlyoutWindowNative.Apply(_windowHandle);
        return _appWindow;
    }

    private void CancelAnimationPreservingValues()
    {
        if (_animation is null) return;
        var opacity = _root.Opacity;
        var x = _translation.X;
        var y = _translation.Y;
        var animation = _animation;
        var completion = _animationCompletion;
        _animation = null;
        _animationCompletion = null;
        animation.Stop();
        _root.Opacity = opacity;
        _translation.X = x;
        _translation.Y = y;
        completion?.TrySetResult(false);
    }

    private Task<bool> AnimateAsync(double opacity, double x, double y, int milliseconds, EasingMode easingMode)
    {
        var completion = new TaskCompletionSource<bool>();
        var easing = new CubicEase { EasingMode = easingMode };
        var duration = new Duration(TimeSpan.FromMilliseconds(milliseconds));
        var storyboard = new Storyboard();
        storyboard.Children.Add(Animation(_root, "Opacity", _root.Opacity, opacity, duration, easing));
        storyboard.Children.Add(Animation(_translation, "X", _translation.X, x, duration, easing));
        storyboard.Children.Add(Animation(_translation, "Y", _translation.Y, y, duration, easing));
        storyboard.Completed += (_, _) =>
        {
            if (!ReferenceEquals(_animation, storyboard)) return;
            _animation = null;
            _animationCompletion = null;
            storyboard.Stop();
            _root.Opacity = opacity;
            _translation.X = x;
            _translation.Y = y;
            completion.TrySetResult(true);
        };
        _animation = storyboard;
        _animationCompletion = completion;
        storyboard.Begin();
        return completion.Task;
    }

    private static DoubleAnimation Animation(
        DependencyObject target,
        string property,
        double from,
        double to,
        Duration duration,
        EasingFunctionBase easing)
    {
        var animation = new DoubleAnimation
        {
            From = from,
            To = to,
            Duration = duration,
            EasingFunction = easing,
            EnableDependentAnimation = true,
        };
        Storyboard.SetTarget(animation, target);
        Storyboard.SetTargetProperty(animation, property);
        return animation;
    }
    public void ShowError(CoreError error) { _error.Title = error.Code; _error.Message = error.Message; _error.IsOpen = true; }
    private async Task RefreshAsync() { _busy.IsActive = true; await _refresh.RefreshAsync(); _busy.IsActive = false; }
    private async Task CalculateAllTimeAsync() { _busy.IsActive = true; await _refresh.CalculateAllTimeAsync(); _busy.IsActive = false; }
}

internal sealed class QuickActionsWindow : Window
{
    private const int WidthEpx = 280;
    private const int HeightEpx = 300;
    private readonly Button _firstButton;
    private bool _closing;

    public QuickActionsWindow(
        Action summary,
        Action refresh,
        Action allTime,
        Action settings,
        Action usage,
        Action exit,
        Action closed)
    {
        Title = "Codex Savings actions";
        SystemBackdrop = new DesktopAcrylicBackdrop();
        ExtendsContentIntoTitleBar = true;
        _firstButton = Button("Show summary", summary);
        var root = new StackPanel
        {
            Width = WidthEpx,
            Height = HeightEpx,
            Padding = new Thickness(12),
            Spacing = 4,
            Children =
            {
                _firstButton,
                Button("Refresh", refresh),
                Button("Calculate all-time total", allTime),
                Button("Settings", settings),
                Button("Open usage dashboard", usage),
                Button("Exit", exit),
            },
        };
        root.KeyDown += (_, eventArgs) =>
        {
            if (eventArgs.Key != Windows.System.VirtualKey.Escape) return;
            eventArgs.Handled = true;
            Dismiss();
        };
        Content = root;
        Activated += (_, eventArgs) =>
        {
            if (eventArgs.WindowActivationState == WindowActivationState.Deactivated) Dismiss();
        };
        Closed += (_, _) => closed();
    }

    public void Present(FlyoutRect? iconRect)
    {
        var window = WinRT.Interop.WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(window));
        appWindow.IsShownInSwitchers = false;
        var presenter = (OverlappedPresenter)appWindow.Presenter;
        presenter.IsResizable = false;
        presenter.IsMaximizable = false;
        presenter.IsMinimizable = false;
        presenter.SetBorderAndTitleBar(false, false);
        presenter.IsAlwaysOnTop = true;
        FlyoutWindowNative.Apply(window);
        FlyoutWindowNative.Place(appWindow, window, iconRect, WidthEpx, HeightEpx, 12, 0);
        Activate();
        _firstButton.Focus(FocusState.Programmatic);
    }

    private Button Button(string text, Action action)
    {
        var button = new Button
        {
            Content = text,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Left,
        };
        button.Click += (_, _) =>
        {
            Dismiss();
            action();
        };
        return button;
    }

    private void Dismiss()
    {
        if (_closing) return;
        _closing = true;
        Close();
    }
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
    public void Present()
    {
        var window = WinRT.Interop.WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(window));
        var scale = FlyoutWindowNative.GetDpi(window) / 96d;
        appWindow.Resize(new SizeInt32(
            (int)Math.Round(520 * scale, MidpointRounding.AwayFromZero),
            (int)Math.Round(600 * scale, MidpointRounding.AwayFromZero)));
        Activate();
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
