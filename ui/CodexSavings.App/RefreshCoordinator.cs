namespace CodexSavings;

internal sealed class RefreshCoordinator : IAsyncDisposable
{
    private readonly CoreBridge _core;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private readonly PeriodicTimer _timer = new(TimeSpan.FromMinutes(5));
    private bool _pending;
    private readonly CancellationTokenSource _cancel = new();
    public SnapshotDto? LastSnapshot { get; private set; }
    public event Action<SnapshotDto>? SnapshotChanged;
    public event Action<CoreError>? Failed;

    public RefreshCoordinator(CoreBridge core) => _core = core;
    public async Task StartAsync()
    {
        _ = Task.Run(async () =>
        {
            await RefreshAsync();
            while (await _timer.WaitForNextTickAsync(_cancel.Token)) await RefreshAsync();
        });
    }
    public async Task RefreshAsync()
    {
        if (!await _gate.WaitAsync(0)) { _pending = true; return; }
        try
        {
            do
            {
                _pending = false;
                var reply = await _core.ScanCurrentAsync();
                if (reply.Ok && reply.Data is not null) { LastSnapshot = reply.Data; SnapshotChanged?.Invoke(reply.Data); }
                else if (reply.Error is not null) Failed?.Invoke(reply.Error);
            } while (_pending);
        }
        finally { _gate.Release(); }
    }
    public ValueTask DisposeAsync() { _cancel.Cancel(); _timer.Dispose(); _cancel.Dispose(); _gate.Dispose(); return ValueTask.CompletedTask; }
}
