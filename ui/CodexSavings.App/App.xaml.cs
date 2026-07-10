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
    private IReadOnlyList<PlanDto> _plans = [];
    private UiStrings? _strings;
    private bool _disposed;
    private UiStrings Strings => _strings ??= new UiStrings("auto");

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
            _settings?.UpdateSnapshot(snapshot);
            _tray?.SetTooltip(SnapshotPresentation.Tooltip(snapshot, _plans));
        });
        _refresh.Failed += error => dispatcher.TryEnqueue(() => _summary?.ShowError(error));
        _tray = new TrayHost();
        _tray.LeftButtonDown += _visibility.BeginTrayInteraction;
        _tray.LeftClicked += ToggleSummaryFromTray;
        _tray.RightClicked += ShowQuickActions;
        var catalog = await _core.LoadSettingsAsync();
        if (catalog.Ok && catalog.Data is not null)
        {
            _plans = catalog.Data.Plans;
            _strings = new UiStrings(catalog.Data.Config.Language);
        }
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
        _summary ??= new SummaryWindow(_refresh!, _plans, Strings, OnSummaryDeactivated, ShowSettings);
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
            Strings,
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
        var settings = new SettingsWindow(_core, _refresh!.LastSnapshot, Strings, savedConfig =>
        {
            _strings = new UiStrings(savedConfig.Language);
            _summary?.Close();
            _summary = null;
            _ = _refresh.RefreshAsync();
        });
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
    private readonly IReadOnlyList<PlanDto> _plans;
    private readonly UiStrings _strings;
    private readonly Action _deactivated;
    private readonly Border _root = new() { Width = WidthEpx, Height = HeightEpx, CornerRadius = new CornerRadius(12) };
    private readonly TranslateTransform _translation = new();
    private readonly TextBlock _plan = new();
    private readonly TextBlock _tier = new();
    private readonly TextBlock _cost = new() { FontSize = 32, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
    private readonly ProgressBar _progress = new() { Minimum = 0, Maximum = 100, Height = 6 };
    private readonly TextBlock _progressText = new() { FontSize = 12, Opacity = .72, VerticalAlignment = VerticalAlignment.Center };
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
    private readonly InfoBar _warning = new() { IsOpen = false, IsClosable = false, Severity = InfoBarSeverity.Warning };
    private readonly InfoBar _error = new() { IsOpen = false, IsClosable = true, Severity = InfoBarSeverity.Error };
    private readonly ProgressRing _busy = new() { IsActive = false, Width = 20, Height = 20 };
    private AppWindow? _appWindow;
    private OverlappedPresenter? _presenter;
    private Storyboard? _animation;
    private TaskCompletionSource<bool>? _animationCompletion;
    private nint _windowHandle;
    private FlyoutPoint _hideOffset;
    private bool _isShown;

    public SummaryWindow(RefreshCoordinator refresh, IReadOnlyList<PlanDto> plans, UiStrings strings, Action deactivated, Action openSettings)
    {
        _refresh = refresh;
        _plans = plans;
        _strings = strings;
        _deactivated = deactivated;
        Title = strings["AppTitle"];
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
        stats.Children.Add(Stat(strings["Today"], _today, 0));
        stats.Children.Add(Stat(strings["UntilReset"], _days, 1));
        stats.Children.Add(Stat(strings["AllTime"], _allTime, 2));
        var settingsButton = new Button
        {
            Content = new FontIcon { Glyph = "\uE713", FontSize = 15 },
            Width = 32,
            Height = 32,
            Padding = new Thickness(0),
            HorizontalAlignment = HorizontalAlignment.Right,
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(settingsButton, strings["Settings"]);
        ToolTipService.SetToolTip(settingsButton, strings["Settings"]);
        settingsButton.Click += (_, _) => openSettings();
        _error.ActionButton = new Button { Content = strings["Retry"] }.Also(button => button.Click += async (_, _) => await RefreshAsync());
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
            new TextBlock { Text = strings["AppTitle"], FontSize = 18, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, VerticalAlignment = VerticalAlignment.Center },
            Chip(_plan), Chip(_tier),
        }});
        Grid.SetColumn(settingsButton, 2);
        header.Children.Add(settingsButton);
        var progressRow = new Grid { ColumnSpacing = 8 };
        progressRow.ColumnDefinitions.Add(new ColumnDefinition());
        progressRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        progressRow.Children.Add(_progress);
        Grid.SetColumn(_progressText, 1);
        progressRow.Children.Add(_progressText);
        var noticeArea = new Grid { Children = { _context, _warning, _error } };
        _root.RenderTransform = _translation;
        _root.Child = new StackPanel { Spacing = 9, Padding = new Thickness(20, 16, 20, 14), Children =
        {
            header,
            _cost, progressRow, noticeArea, stats,
            new Grid { Children =
            {
                _updated,
                new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Right, Children =
                {
                    _busy,
                    new Button { Content = strings["Refresh"] }.Also(button => button.Click += async (_, _) => await RefreshAsync()),
                }},
            }},
        }};
        Content = _root;
        if (_refresh.LastSnapshot is not null) Update(_refresh.LastSnapshot); else _ = RefreshAsync();
    }
    public void Update(SnapshotDto snapshot)
    {
        var planUsd = SnapshotPresentation.PlanUsd(snapshot.Config, _plans);
        _plan.Text = SnapshotPresentation.PlanName(snapshot.Config, _plans);
        _tier.Text = snapshot.ServiceTier;
        _cost.Text = "$" + snapshot.Cycle.CostUsd.ToString("F2");
        _progress.Visibility = planUsd > 0 ? Visibility.Visible : Visibility.Collapsed;
        _progressText.Visibility = _progress.Visibility;
        var percentage = planUsd > 0 ? snapshot.Cycle.CostUsd / planUsd * 100 : 0;
        _progress.Value = Math.Min(100, percentage);
        _progressText.Text = percentage.ToString("F0") + "%";
        _context.Text = SnapshotPresentation.Context(snapshot, _plans);
        _today.Text = "$" + snapshot.Today.CostUsd.ToString("F2");
        _days.Text = snapshot.DaysUntilReset.ToString();
        _allTime.Content = snapshot.AllTime is null ? _strings["Calculate"] : "$" + snapshot.AllTime.CostUsd.ToString("F2");
        _updated.Text = _strings.Format("UpdatedFormat", snapshot.UpdatedAt);
        _error.IsOpen = false;
        var warning = SnapshotPresentation.Warning(snapshot);
        _warning.Title = _strings["ReviewEstimate"];
        _warning.Message = warning ?? string.Empty;
        _warning.IsOpen = warning is not null;
        _context.Visibility = warning is null ? Visibility.Visible : Visibility.Collapsed;
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
    public void ShowError(CoreError error)
    {
        _warning.IsOpen = false;
        _context.Visibility = Visibility.Collapsed;
        _error.Title = error.Code;
        _error.Message = error.Message;
        _error.IsOpen = true;
    }
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
        UiStrings strings,
        Action summary,
        Action refresh,
        Action allTime,
        Action settings,
        Action usage,
        Action exit,
        Action closed)
    {
        Title = strings["ActionsTitle"];
        SystemBackdrop = new DesktopAcrylicBackdrop();
        ExtendsContentIntoTitleBar = true;
        _firstButton = Button(strings["ShowSummary"], summary);
        var root = new StackPanel
        {
            Width = WidthEpx,
            Height = HeightEpx,
            Padding = new Thickness(12),
            Spacing = 4,
            Children =
            {
                _firstButton,
                Button(strings["Refresh"], refresh),
                Button(strings["CalculateAllTime"], allTime),
                Button(strings["Settings"], settings),
                Button(strings["OpenUsage"], usage),
                Button(strings["Exit"], exit),
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
    private readonly CoreBridge _core;
    private SnapshotDto? _snapshot;
    private readonly UiStrings _strings;
    private readonly Action<ConfigDto> _saved;
    private readonly ComboBox _plans = new() { HorizontalAlignment = HorizontalAlignment.Stretch, DisplayMemberPath = nameof(PlanChoice.Display) };
    private readonly TextBlock _planDetails = new() { Opacity = .72, TextWrapping = TextWrapping.Wrap };
    private readonly NumberBox _amount = new()
    {
        Minimum = 0,
        Maximum = 1_000_000,
        SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Inline,
    };
    private readonly StackPanel _amountSection;
    private readonly NumberBox _day = new()
    {
        Minimum = 1,
        Maximum = 31,
        SmallChange = 1,
        SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Inline,
    };
    private readonly TextBlock _nextReset = new() { Opacity = .72 };
    private readonly TextBox _codexHome = new() { IsReadOnly = true, TextWrapping = TextWrapping.Wrap };
    private readonly TextBlock _tier = new() { Text = "standard", Opacity = .72 };
    private readonly ComboBox _language = new() { HorizontalAlignment = HorizontalAlignment.Stretch, DisplayMemberPath = nameof(LanguageChoice.Display) };
    private readonly Button _save = new() { IsEnabled = false };
    private readonly InfoBar _error = new() { IsOpen = false, IsClosable = true, Severity = InfoBarSeverity.Error };
    private readonly Grid _root = new();
    private ConfigDto? _initial;
    private bool _loading = true;
    private bool _allowClose;
    private bool _dialogOpen;
    private int _previewGeneration;
    private AppWindow? _appWindow;

    public SettingsWindow(CoreBridge core, SnapshotDto? snapshot, UiStrings strings, Action<ConfigDto> saved)
    {
        _core = core;
        _snapshot = snapshot;
        _strings = strings;
        _saved = saved;
        SystemBackdrop = new MicaBackdrop();
        Title = strings["SettingsTitle"];
        _save.Content = strings["Save"];
        _amountSection = new StackPanel { Spacing = 6, Children =
        {
            Label(strings["CustomAmount"]), _amount,
        }};
        var form = new StackPanel { Padding = new Thickness(24, 20, 24, 16), Spacing = 10, Children =
        {
            Heading(strings["Subscription"]), _plans, _planDetails, _amountSection,
            Heading(strings["BillingCycle"]), Label(strings["CycleDay"]), _day,
            new TextBlock { Text = strings["ShortMonths"], Opacity = .72, TextWrapping = TextWrapping.Wrap },
            _nextReset,
            Heading(strings["Language"]), _language,
            Heading(strings["DataAdvanced"]),
            Label("CODEX_HOME"),
            _codexHome,
            Label(strings["PricingTier"]),
            _tier,
            new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children =
            {
                new Button { Content = strings["OpenConfig"] }.Also(button => button.Click += (_, _) => OpenConfig()),
                new Button { Content = strings["OpenUsage"] }.Also(button => button.Click += (_, _) => OpenUsage()),
            }},
            _error,
        }};
        var cancel = new Button { Content = strings["Cancel"] };
        cancel.Click += async (_, _) => await RequestCloseAsync();
        _save.Click += async (_, _) => await SaveAsync();
        var footer = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            HorizontalAlignment = HorizontalAlignment.Right,
            Spacing = 8,
            Padding = new Thickness(24, 12, 24, 18),
            Children = { _save, cancel },
        };
        _root.RowDefinitions.Add(new RowDefinition());
        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        _root.Children.Add(new ScrollViewer { Content = form, VerticalScrollBarVisibility = ScrollBarVisibility.Auto });
        Grid.SetRow(footer, 1);
        _root.Children.Add(footer);
        _root.KeyDown += async (_, eventArgs) =>
        {
            if (eventArgs.Key != Windows.System.VirtualKey.Escape) return;
            eventArgs.Handled = true;
            await RequestCloseAsync();
        };
        Content = _root;

        _plans.SelectionChanged += (_, _) => UpdateState();
        _amount.ValueChanged += (_, _) => UpdateState();
        _day.ValueChanged += (_, _) => UpdateState();
        _language.SelectionChanged += (_, _) => UpdateState();
        if (snapshot is not null) UpdateSnapshot(snapshot);
        _ = LoadAsync();
    }
    public void Present()
    {
        var window = WinRT.Interop.WindowNative.GetWindowHandle(this);
        _appWindow ??= AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(window));
        _appWindow.Closing -= OnClosing;
        _appWindow.Closing += OnClosing;
        var scale = FlyoutWindowNative.GetDpi(window) / 96d;
        _appWindow.Resize(new SizeInt32(
            (int)Math.Round(520 * scale, MidpointRounding.AwayFromZero),
            (int)Math.Round(600 * scale, MidpointRounding.AwayFromZero)));
        Activate();
    }

    public void UpdateSnapshot(SnapshotDto snapshot)
    {
        _snapshot = snapshot;
        _codexHome.Text = snapshot.CodexHome;
        _tier.Text = snapshot.ServiceTier;
        if (!_loading && !HasChanges() && !string.IsNullOrWhiteSpace(snapshot.CycleNext))
            _nextReset.Text = _strings.Format("CurrentNextResetFormat", snapshot.CycleNext);
    }
    private async Task LoadAsync()
    {
        var reply = await _core.LoadSettingsAsync();
        if (!reply.Ok || reply.Data is null)
        {
            ShowError(reply.Error ?? new CoreError("settings_error", _strings["SettingsLoadError"]));
            return;
        }

        _initial = reply.Data.Config;
        var spanish = SnapshotPresentation.UsesSpanish(_initial);
        var plans = reply.Data.Plans.Select(plan => new PlanChoice(
            plan,
            (spanish ? plan.NameEs : plan.NameEn) + (plan.Usd > 0 ? " — " + _strings.Format("PerMonthFormat", plan.Usd.ToString("F0")) : string.Empty))).ToArray();
        var languages = new[]
        {
            new LanguageChoice("auto", _strings["Automatic"]),
            new LanguageChoice("en", _strings["English"]),
            new LanguageChoice("es", _strings["Spanish"]),
        };
        _plans.ItemsSource = plans;
        _plans.SelectedItem = plans.First(choice => choice.Plan.Id == _initial.Plan);
        _amount.Value = _initial.MonthlyUsdOverride ?? 0;
        _day.Value = _initial.CycleDay;
        _language.ItemsSource = languages;
        _language.SelectedItem = languages.First(choice => choice.Id == _initial.Language);
        _nextReset.Text = string.IsNullOrWhiteSpace(_snapshot?.CycleNext)
            ? _strings["NextResetPending"]
            : _strings.Format("CurrentNextResetFormat", _snapshot.CycleNext);
        _loading = false;
        UpdateState();
    }

    private void UpdateState()
    {
        if (_loading || _plans.SelectedItem is not PlanChoice choice) return;
        var spanish = _initial is not null && SnapshotPresentation.UsesSpanish(_initial);
        _planDetails.Text = spanish ? choice.Plan.LimitsEs : choice.Plan.LimitsEn;
        _amountSection.Visibility = choice.Plan.Id == "custom" ? Visibility.Visible : Visibility.Collapsed;
        var config = CurrentConfig();
        _save.IsEnabled = config is not null && config != _initial;
        var generation = ++_previewGeneration;
        if (config is not null) _ = UpdatePreviewAsync(config, generation);
    }

    private async Task UpdatePreviewAsync(ConfigDto config, int generation)
    {
        var reply = await _core.PreviewCycleAsync(config);
        if (generation != _previewGeneration || !reply.Ok || reply.Data is null) return;
        _nextReset.Text = _strings.Format(
            "NextResetPreviewFormat",
            reply.Data.CycleNext,
            reply.Data.DaysUntilReset);
    }

    private ConfigDto? CurrentConfig() =>
        _plans.SelectedItem is PlanChoice plan && _language.SelectedItem is LanguageChoice language
            ? SettingsValidator.TryCreate(plan.Plan.Id, _amount.Value, language.Id, _day.Value)
            : null;

    private async Task SaveAsync()
    {
        var config = CurrentConfig();
        if (config is null || config == _initial) return;
        _save.IsEnabled = false;
        _error.IsOpen = false;
        var reply = await _core.SaveSettingsAsync(config);
        if (reply.Ok && reply.Data is not null)
        {
            _initial = reply.Data;
            _allowClose = true;
            _saved(reply.Data);
            Close();
            return;
        }
        ShowError(reply.Error ?? new CoreError("settings_error", _strings["SettingsSaveError"]));
        UpdateState();
    }

    private void OnClosing(AppWindow sender, AppWindowClosingEventArgs eventArgs)
    {
        if (_allowClose || !HasChanges()) return;
        eventArgs.Cancel = true;
        _ = ConfirmDiscardAsync();
    }

    private async Task RequestCloseAsync()
    {
        if (!HasChanges())
        {
            _allowClose = true;
            Close();
            return;
        }
        await ConfirmDiscardAsync();
    }

    private async Task ConfirmDiscardAsync()
    {
        if (_dialogOpen) return;
        _dialogOpen = true;
        try
        {
            var dialog = new ContentDialog
            {
                XamlRoot = _root.XamlRoot,
                Title = _strings["DiscardTitle"],
                Content = _strings["DiscardMessage"],
                PrimaryButtonText = _strings["Discard"],
                CloseButtonText = _strings["KeepEditing"],
                DefaultButton = ContentDialogButton.Close,
            };
            if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
            _allowClose = true;
            Close();
        }
        finally { _dialogOpen = false; }
    }

    private bool HasChanges() => !_loading && CurrentConfig() != _initial;

    private void OpenConfig()
    {
        var path = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "Codex Savings Tracker",
            "config.json");
        Open(path, "config_missing", _strings["ConfigMissing"]);
    }

    private void OpenUsage() => Open(
        "https://chatgpt.com/codex/cloud/settings/analytics",
        "usage_error",
        _strings["UsageError"]);

    private void Open(string target, string code, string message)
    {
        try
        {
            if (!target.StartsWith("https://", StringComparison.Ordinal) && !File.Exists(target))
            {
                ShowError(new CoreError(code, message));
                return;
            }
            Process.Start(new ProcessStartInfo(target) { UseShellExecute = true });
        }
        catch { ShowError(new CoreError(code, message)); }
    }

    private void ShowError(CoreError error)
    {
        _error.Title = error.Code;
        _error.Message = error.Message;
        _error.IsOpen = true;
    }

    private static TextBlock Heading(string text) => new()
    {
        Text = text,
        FontSize = 18,
        FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
        Margin = new Thickness(0, 8, 0, 0),
    };

    private static TextBlock Label(string text) => new() { Text = text };

    private sealed record PlanChoice(PlanDto Plan, string Display);
    private sealed record LanguageChoice(string Id, string Display);
}
internal static class ControlExtensions { internal static T Also<T>(this T control, Action<T> configure) where T : Control { configure(control); return control; } }
