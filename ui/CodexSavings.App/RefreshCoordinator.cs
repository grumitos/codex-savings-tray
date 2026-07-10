namespace CodexSavings;

internal sealed class RefreshCoordinator : IAsyncDisposable
{
    private readonly CoreBridge _core;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly PeriodicTimer _timer = new(TimeSpan.FromMinutes(5));
    private int _pending;
    private readonly CancellationTokenSource _cancel = new();
    private Task? _loop;
    public SnapshotDto? LastSnapshot { get; private set; }
    public event Action<SnapshotDto>? SnapshotChanged;
    public event Action<CoreError>? Failed;

    public RefreshCoordinator(CoreBridge core) => _core = core;
    public Task StartAsync()
    {
        _loop ??= RunAsync();
        return Task.CompletedTask;
    }

    private async Task RunAsync()
    {
        try
        {
            await RefreshAsync();
            while (await _timer.WaitForNextTickAsync(_cancel.Token)) await RefreshAsync();
        }
        catch (OperationCanceledException) when (_cancel.IsCancellationRequested) { }
    }

    public async Task RefreshAsync()
    {
        if (!await _gate.WaitAsync(0)) { Interlocked.Exchange(ref _pending, 1); return; }
        var rerun = false;
        try
        {
            do
            {
                Interlocked.Exchange(ref _pending, 0);
                var reply = await _core.ScanCurrentAsync();
                if (reply.Ok && reply.Data is not null)
                {
                    LastSnapshot = PreserveAllTime(reply.Data, LastSnapshot);
                    SnapshotChanged?.Invoke(LastSnapshot);
                }
                else if (reply.Error is not null) Failed?.Invoke(reply.Error);
            } while (Interlocked.Exchange(ref _pending, 0) != 0);
        }
        finally
        {
            _gate.Release();
            rerun = Interlocked.Exchange(ref _pending, 0) != 0;
        }
        if (rerun) await RefreshAsync();
    }

    public async Task CalculateAllTimeAsync()
    {
        await _gate.WaitAsync();
        var rerun = false;
        try
        {
            var reply = await _core.ScanAllTimeAsync();
            if (reply.Ok && reply.Data is not null)
            {
                LastSnapshot = reply.Data;
                SnapshotChanged?.Invoke(reply.Data);
            }
            else if (reply.Error is not null) Failed?.Invoke(reply.Error);
        }
        finally
        {
            _gate.Release();
            rerun = Interlocked.Exchange(ref _pending, 0) != 0;
        }
        if (rerun) await RefreshAsync();
    }

    internal static SnapshotDto PreserveAllTime(SnapshotDto current, SnapshotDto? previous) =>
        current.AllTime is null && previous?.AllTime is not null
            ? current with { AllTime = previous.AllTime, AllTimeUpdatedAt = previous.AllTimeUpdatedAt }
            : current;

    public async ValueTask DisposeAsync()
    {
        _cancel.Cancel();
        if (_loop is not null) await _loop;
        _timer.Dispose();
        _cancel.Dispose();
        _gate.Dispose();
    }
}
