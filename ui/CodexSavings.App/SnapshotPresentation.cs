using System.Globalization;

namespace CodexSavings;

internal static class SnapshotPresentation
{
    internal static double PlanUsd(ConfigDto config, IReadOnlyList<PlanDto> plans) =>
        config.MonthlyUsdOverride ?? plans.FirstOrDefault(plan => plan.Id == config.Plan)?.Usd ?? 0;

    internal static string PlanName(ConfigDto config, IReadOnlyList<PlanDto> plans)
    {
        var plan = plans.FirstOrDefault(item => item.Id == config.Plan);
        return plan is null
            ? config.Plan.Replace('_', ' ')
            : UsesSpanish(config) ? plan.NameEs : plan.NameEn;
    }

    internal static string Context(SnapshotDto snapshot, IReadOnlyList<PlanDto> plans)
    {
        var spanish = UsesSpanish(snapshot.Config);
        var plan = plans.FirstOrDefault(item => item.Id == snapshot.Config.Plan);
        var planUsd = SnapshotPresentation.PlanUsd(snapshot.Config, plans);
        if (planUsd <= 0)
            return plan is null
                ? spanish ? "Valor equivalente de API en este ciclo" : "API-equivalent value this cycle"
                : spanish ? plan.LimitsEs : plan.LimitsEn;

        var difference = Math.Abs(snapshot.Cycle.CostUsd - planUsd).ToString("F2", CultureInfo.InvariantCulture);
        return snapshot.Cycle.CostUsd >= planUsd
            ? spanish ? $"${difference} ahorrados sobre el precio del plan" : $"${difference} saved beyond the plan price"
            : spanish ? $"Faltan ${difference} para amortizar el plan" : $"${difference} left to break even";
    }

    internal static string Tooltip(SnapshotDto snapshot, IReadOnlyList<PlanDto> plans)
    {
        var spanish = UsesSpanish(snapshot.Config);
        var parts = new List<string>
        {
            $"{PlanName(snapshot.Config, plans)} · {(spanish ? "ciclo" : "cycle")} ${snapshot.Cycle.CostUsd.ToString("F2", CultureInfo.InvariantCulture)}",
        };
        var planUsd = PlanUsd(snapshot.Config, plans);
        if (planUsd > 0)
            parts.Add((snapshot.Cycle.CostUsd / planUsd * 100).ToString("F0", CultureInfo.InvariantCulture) + "%");
        if (snapshot.AllTime is not null)
            parts.Add($"{(spanish ? "historico" : "all time")} ${snapshot.AllTime.CostUsd.ToString("F2", CultureInfo.InvariantCulture)}");
        return string.Join(" · ", parts);
    }

    internal static string? Warning(SnapshotDto snapshot)
    {
        if (snapshot.UnknownModels.Count == 0 && snapshot.AssumedModels == 0) return null;
        var spanish = UsesSpanish(snapshot.Config);
        var parts = new List<string>();
        if (snapshot.UnknownModels.Count > 0)
        {
            var models = string.Join(", ", snapshot.UnknownModels.Take(3));
            if (snapshot.UnknownModels.Count > 3) models += ", ...";
            parts.Add(spanish ? $"Sin precio: {models}" : $"No price: {models}");
        }
        if (snapshot.AssumedModels > 0)
            parts.Add(spanish
                ? $"{snapshot.AssumedModels} sesiones usaron un modelo supuesto"
                : $"{snapshot.AssumedModels} sessions used an assumed model");
        return string.Join(". ", parts);
    }

    internal static bool UsesSpanish(ConfigDto config) =>
        config.Language == "es" ||
        config.Language == "auto" && CultureInfo.CurrentUICulture.TwoLetterISOLanguageName == "es";
}
