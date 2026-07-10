using System.Runtime.InteropServices;
using System.Text.Json;

namespace CodexSavings;

internal sealed class CoreBridge
{
    private static readonly JsonSerializerOptions Json = new() { PropertyNameCaseInsensitive = true };

    public Task<CoreEnvelope<SnapshotDto>> ScanCurrentAsync() => Task.Run(() => Read<SnapshotDto>(cst_scan_current));
    public Task<CoreEnvelope<SnapshotDto>> ScanAllTimeAsync() => Task.Run(() => Read<SnapshotDto>(cst_scan_all_time));
    public Task<CoreEnvelope<SettingsCatalogDto>> LoadSettingsAsync() => Task.Run(() => Read<SettingsCatalogDto>(cst_load_settings));
    public Task<CoreEnvelope<CyclePreviewDto>> PreviewCycleAsync(ConfigDto config) =>
        Task.Run(() => Read<CyclePreviewDto>(() => cst_preview_cycle(JsonSerializer.Serialize(config, Json))));
    public Task<CoreEnvelope<ConfigDto>> SaveSettingsAsync(ConfigDto config) =>
        Task.Run(() => Read<ConfigDto>(() => cst_save_settings(JsonSerializer.Serialize(config, Json))));

    private static CoreEnvelope<T> Read<T>(Func<nint> call)
    {
        var pointer = call();
        if (pointer == 0) return new(false, default, new("internal_error", "The core returned no response."));
        try
        {
            return JsonSerializer.Deserialize<CoreEnvelope<T>>(Marshal.PtrToStringUTF8(pointer)!, Json)
                ?? new(false, default, new("invalid_response", "The core returned invalid JSON."));
        }
        finally { cst_free_string(pointer); }
    }

    [DllImport("codex_savings_core.dll", CallingConvention = CallingConvention.Cdecl)]
    private static extern nint cst_scan_current();
    [DllImport("codex_savings_core.dll", CallingConvention = CallingConvention.Cdecl)]
    private static extern nint cst_scan_all_time();
    [DllImport("codex_savings_core.dll", CallingConvention = CallingConvention.Cdecl)]
    private static extern nint cst_load_settings();
    [DllImport("codex_savings_core.dll", CallingConvention = CallingConvention.Cdecl)]
    private static extern nint cst_preview_cycle([MarshalAs(UnmanagedType.LPUTF8Str)] string json);
    [DllImport("codex_savings_core.dll", CallingConvention = CallingConvention.Cdecl)]
    private static extern nint cst_save_settings([MarshalAs(UnmanagedType.LPUTF8Str)] string json);
    [DllImport("codex_savings_core.dll", CallingConvention = CallingConvention.Cdecl)]
    private static extern void cst_free_string(nint pointer);
}

internal sealed record CoreEnvelope<T>(bool Ok, T? Data, CoreError? Error);
internal sealed record CoreError(string Code, string Message);
internal sealed record ConfigDto(string Plan, double? MonthlyUsdOverride, string Language, ushort CycleDay);
internal sealed record PlanDto(string Id, string NameEn, string NameEs, double Usd, string LimitsEn, string LimitsEs);
internal sealed record SettingsCatalogDto(ConfigDto Config, IReadOnlyList<PlanDto> Plans);
internal sealed record CyclePreviewDto(string CycleNext, int DaysUntilReset);
internal sealed record PeriodDto(double CostUsd, uint Calls, uint Sessions, ulong InputTokens, ulong CachedInputTokens, ulong CacheWriteTokens, ulong OutputTokens, ulong ReasoningOutputTokens, ulong TotalTokens);
internal sealed record SnapshotDto(PeriodDto Cycle, PeriodDto Today, PeriodDto? AllTime, ConfigDto Config, string ServiceTier, string CycleStart, string CycleNext, int DaysUntilReset, string UpdatedAt, string AllTimeUpdatedAt, string CodexHome, IReadOnlyList<string> UnknownModels, uint AssumedModels, string? Error);
