using Microsoft.Windows.ApplicationModel.Resources;
using System.Globalization;

namespace CodexSavings;

internal sealed class UiStrings
{
    private readonly ResourceMap _resources;
    private readonly ResourceContext _context;

    internal UiStrings(string language)
    {
        var manager = new ResourceManager();
        _resources = manager.MainResourceMap.GetSubtree("Resources");
        _context = manager.CreateResourceContext();
        if (language == "en") _context.QualifierValues["Language"] = "en-US";
        if (language == "es") _context.QualifierValues["Language"] = "es-ES";
    }

    internal string this[string key] => _resources.GetValue(key, _context).ValueAsString;

    internal string Format(string key, params object[] values) =>
        string.Format(CultureInfo.CurrentCulture, this[key], values);
}
