namespace CodexSavings;

internal static class SettingsValidator
{
    internal static ConfigDto? TryCreate(string plan, double amount, string language, double cycleDay)
    {
        if (!double.IsFinite(cycleDay) || cycleDay != Math.Truncate(cycleDay) || cycleDay is < 1 or > 31)
            return null;
        var monthlyAmount = plan == "custom" && double.IsFinite(amount) ? amount : (double?)null;
        var config = new ConfigDto(plan, monthlyAmount, language, (ushort)cycleDay);
        return IsValid(config) ? config : null;
    }

    internal static bool IsValid(ConfigDto config) =>
        config.CycleDay is >= 1 and <= 31 &&
        config.Language is "auto" or "en" or "es" &&
        (config.MonthlyUsdOverride is null || config.MonthlyUsdOverride is >= 0 and <= 1_000_000) &&
        (config.Plan != "custom" || config.MonthlyUsdOverride is not null);
}
