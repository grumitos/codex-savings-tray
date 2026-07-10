namespace CodexSavings;

internal static class SettingsValidator
{
    internal static bool IsValid(ConfigDto config) =>
        config.CycleDay is >= 1 and <= 31 &&
        config.Language is "auto" or "en" or "es" &&
        (config.MonthlyUsdOverride is null || config.MonthlyUsdOverride is >= 0 and <= 1_000_000) &&
        (config.Plan != "custom" || config.MonthlyUsdOverride is not null);
}
