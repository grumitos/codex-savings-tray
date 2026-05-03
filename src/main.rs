#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    cmp::Ordering as CmpOrdering,
    collections::{HashMap, HashSet},
    env,
    ffi::c_void,
    fs::{self, File},
    io::{BufRead, BufReader},
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    slice,
    sync::{
        atomic::{AtomicU16, Ordering},
        Mutex, OnceLock,
    },
};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, SYSTEMTIME, WPARAM},
    Globalization::GetUserDefaultUILanguage,
    Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_NONE, DWMWA_BORDER_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    },
    Graphics::Gdi::{
        BeginPaint, CreateBitmap, CreateDIBSection, CreateFontW, CreateRoundRectRgn,
        CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FillRgn, GetMonitorInfoW,
        GetStockObject, GetTextExtentPoint32W, InvalidateRect, MonitorFromPoint, SelectObject,
        SetBkMode, SetTextColor, SetWindowRgn, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DEFAULT_GUI_FONT, DIB_RGB_COLORS, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
        DT_RIGHT, DT_SINGLELINE, HBITMAP, HDC, HRGN, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        TRANSPARENT,
    },
    Graphics::GdiPlus::{
        CompositingModeSourceOver, CompositingQualityHighQuality, DashCapFlat, FillModeWinding,
        GdipAddPathArc, GdipAddPathLine, GdipBitmapGetPixel, GdipClosePathFigure,
        GdipCreateBitmapFromHICON, GdipCreateFromHDC, GdipCreatePath, GdipCreatePen1,
        GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen,
        GdipDisposeImage, GdipDrawArc, GdipDrawPath, GdipFillPath, GdipSetCompositingMode,
        GdipSetCompositingQuality, GdipSetPenLineCap197819, GdipSetPixelOffsetMode,
        GdipSetSmoothingMode, GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, GpBitmap,
        GpBrush, GpGraphics, GpImage, GpPath, GpPen, GpSolidFill, LineCapRound, Ok as GdipOk,
        PixelOffsetModeHalf, SmoothingModeAntiAlias, UnitPixel,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
        SystemInformation::GetLocalTime,
    },
    UI::{
        HiDpi::{
            GetDpiForWindow, SetProcessDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
        Shell::{
            ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
            NIM_MODIFY, NOTIFYICONDATAW,
        },
        WindowsAndMessaging::*,
    },
};

const WM_TRAYICON: u32 = WM_USER + 7;
const TRAY_UID: u32 = 1;
const TIMER_UID: usize = 1;
const ID_REFRESH: usize = 1001;
const ID_EXIT: usize = 1002;
const ID_ALL_TIME: usize = 1003;
const ID_OPEN_CONFIG: usize = 1004;
const ID_OPEN_USAGE: usize = 1005;
const ID_CYCLE_DAY: usize = 1006;
const ID_PLAN_BASE: usize = 1100;
const VK_ESCAPE_CODE: u32 = 0x1b;
const VK_RETURN_CODE: u32 = 0x0d;
const POPUP_W: i32 = 392;
const POPUP_H: i32 = 226;
const POPUP_GAP: i32 = 12;
const CONTENT_PAD: i32 = 24;
const DIALOG_W: i32 = 330;
const DIALOG_H: i32 = 388;
const WINDOW_RADIUS: i32 = 16;
const CONTROL_RADIUS: i32 = 7;
const DWM_COLOR_NONE: COLORREF = 0xffff_fffe;
const DWM_WINDOW_CORNER_DONOTROUND: i32 = 1;
const USAGE_URL: &str = "https://chatgpt.com/codex/cloud/settings/analytics";

#[derive(Clone, Copy)]
struct Price {
    input: f64,
    cached: f64,
    output: f64,
}

const fn price(input: f64, cached: f64, output: f64) -> Price {
    Price {
        input,
        cached,
        output,
    }
}

#[rustfmt::skip]
const PRICES: &[(&str, Price)] = &[
    ("gpt-5.5", price(5.0, 0.5, 30.0)),
    ("gpt-5.4-mini", price(0.75, 0.075, 4.5)),
    ("gpt-5.4-nano", price(0.20, 0.02, 1.25)),
    ("gpt-5.4", price(2.5, 0.25, 15.0)),
    ("gpt-5.3-codex", price(1.75, 0.175, 14.0)),
    ("gpt-5.2-codex", price(1.75, 0.175, 14.0)),
    ("gpt-5.1-codex-max", price(1.25, 0.125, 10.0)),
    ("gpt-5.1-codex", price(1.25, 0.125, 10.0)),
    ("gpt-5.1", price(1.25, 0.125, 10.0)),
    ("gpt-5", price(1.25, 0.125, 10.0)),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Lang {
    En,
    Es,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ServiceTier {
    #[default]
    Standard,
    Fast,
}

impl ServiceTier {
    fn from_str(value: &str) -> Option<Self> {
        match value
            .trim()
            .trim_matches(['"', '\''])
            .to_ascii_lowercase()
            .as_str()
        {
            "standard" => Some(Self::Standard),
            "fast" => Some(Self::Fast),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct Plan {
    id: &'static str,
    en: &'static str,
    es: &'static str,
    usd: f64,
    limits_en: &'static str,
    limits_es: &'static str,
}

const fn plan(
    id: &'static str,
    en: &'static str,
    es: &'static str,
    usd: f64,
    limits_en: &'static str,
    limits_es: &'static str,
) -> Plan {
    Plan {
        id,
        en,
        es,
        usd,
        limits_en,
        limits_es,
    }
}

#[rustfmt::skip]
const PLANS: &[Plan] = &[
    plan("free", "Free", "Gratis", 0.0, "Quick tasks; see dashboard", "Tareas rapidas; ver panel"),
    plan("go", "Go", "Go", 8.0, "Lightweight tasks; see dashboard", "Tareas ligeras; ver panel"),
    plan("plus", "Plus", "Plus", 20.0, "5h local: 5.5 15-80, 5.4 20-100", "5h local: 5.5 15-80, 5.4 20-100"),
    plan("pro_5x", "Pro 5x", "Pro 5x", 100.0, "5h local: 5.5 80-400, 5.4 100-500", "5h local: 5.5 80-400, 5.4 100-500"),
    plan("pro_20x", "Pro 20x", "Pro 20x", 200.0, "5h local: 5.5 300-1600, 5.4 400-2000", "5h local: 5.5 300-1600, 5.4 400-2000"),
    plan("business", "Business", "Business", 0.0, "Pay as you go; seats often match Plus", "Pago por uso; asientos suelen igualar Plus"),
    plan("enterprise_edu", "Enterprise/Edu", "Enterprise/Edu", 0.0, "Credits or Plus-like seats", "Creditos o asientos tipo Plus"),
    plan("api_key", "API Key", "Clave API", 0.0, "Usage-based API pricing", "Precio API por uso"),
    plan("custom", "Custom", "Personalizado", 0.0, "Manual monthly amount", "Monto mensual manual"),
];

const CREDIT_RATES: &str =
    "Credits/1M tokens: 5.5 125/12.5/750; 5.4 62.5/6.25/375; mini 18.75/1.875/113; fast 5.5 x2.5, 5.4 x2";

#[derive(Clone)]
struct Config {
    plan: String,
    monthly_usd_override: Option<f64>,
    language: String,
    cycle_day: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            plan: "plus".to_string(),
            monthly_usd_override: None,
            language: "auto".to_string(),
            cycle_day: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
struct DateKey {
    year: u16,
    month: u16,
    day: u16,
}

#[derive(Clone, Copy, Default)]
struct Usage {
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cached += other.cached;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.total += other.total;
    }

    fn delta(self, previous: Usage) -> Usage {
        Usage {
            input: self.input.saturating_sub(previous.input),
            cached: self.cached.saturating_sub(previous.cached),
            output: self.output.saturating_sub(previous.output),
            reasoning: self.reasoning.saturating_sub(previous.reasoning),
            total: self.total.saturating_sub(previous.total),
        }
    }

    fn any(self) -> bool {
        self.input + self.cached + self.output + self.reasoning + self.total > 0
    }
}

#[derive(Clone, Default)]
struct Period {
    usage: Usage,
    cost: f64,
    calls: u32,
    sessions: u32,
}

impl Period {
    fn add(&mut self, other: Period) {
        self.usage.add(other.usage);
        self.cost += other.cost;
        self.calls += other.calls;
        self.sessions += other.sessions;
    }
}

#[derive(Clone)]
struct Snapshot {
    month: Period,
    today: Period,
    all_time: Option<Period>,
    config: Config,
    updated: String,
    all_time_updated: String,
    codex_home: PathBuf,
    service_tier: ServiceTier,
    cycle_start: DateKey,
    cycle_next: DateKey,
    cycle_days_left: i32,
    unknown_models: Vec<String>,
    assumed_models: u32,
    error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            month: Period::default(),
            today: Period::default(),
            all_time: None,
            config: Config::default(),
            updated: String::new(),
            all_time_updated: String::new(),
            codex_home: codex_home(),
            service_tier: ServiceTier::Standard,
            cycle_start: DateKey::default(),
            cycle_next: DateKey::default(),
            cycle_days_left: 0,
            unknown_models: vec![],
            assumed_models: 0,
            error: None,
        }
    }
}

static SNAPSHOT: OnceLock<Mutex<Snapshot>> = OnceLock::new();
static CYCLE_DIALOG_DAY: AtomicU16 = AtomicU16::new(1);

#[derive(Clone, Copy)]
struct Ui {
    hdc: HDC,
    dpi: i32,
}

#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    bg: u32,
    card: u32,
    card_alt: u32,
    border_argb: u32,
    text: COLORREF,
    muted: COLORREF,
    subtle: COLORREF,
    track: COLORREF,
    accent_argb: u32,
    danger: COLORREF,
}

fn system_theme() -> Theme {
    if windows_apps_dark() {
        Theme {
            dark: true,
            bg: argb(255, 28, 33, 43),
            card: argb(255, 35, 41, 53),
            card_alt: argb(255, 42, 49, 63),
            border_argb: argb(118, 95, 106, 128),
            text: rgb(243, 246, 250),
            muted: rgb(190, 198, 210),
            subtle: rgb(137, 149, 166),
            track: rgb(52, 61, 78),
            accent_argb: argb(255, 75, 119, 255),
            danger: rgb(231, 121, 99),
        }
    } else {
        Theme {
            dark: false,
            bg: argb(255, 246, 250, 255),
            card: argb(255, 255, 255, 255),
            card_alt: argb(255, 236, 242, 252),
            border_argb: argb(150, 194, 205, 219),
            text: rgb(19, 24, 33),
            muted: rgb(82, 91, 106),
            subtle: rgb(118, 129, 145),
            track: rgb(218, 226, 236),
            accent_argb: argb(255, 74, 112, 245),
            danger: rgb(190, 91, 76),
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--once") {
        print_once(args.iter().any(|arg| arg == "--all-time"));
        return;
    }
    unsafe { run_tray() };
}

fn print_once(include_all_time: bool) {
    let config = load_config();
    let mut snap = scan_month(&config).unwrap_or_else(|error| Snapshot {
        error: Some(error),
        config: config.clone(),
        ..Snapshot::default()
    });
    if include_all_time {
        calculate_all_time(&mut snap);
    }
    let plan = current_plan(&snap.config);
    let plan_usd = plan_usd(&snap.config);
    println!("Codex savings tray");
    println!("Home: {}", snap.codex_home.display());
    println!("Plan: {} ({}/month)", plan.en, money(plan_usd));
    println!("Pricing tier: {}", service_tier_label(snap.service_tier));
    println!(
        "Plan cycle: day {} (current starts {}, next reset {})",
        snap.config.cycle_day,
        format_date(snap.cycle_start),
        format_date(snap.cycle_next)
    );
    if plan_usd > 0.0 {
        println!(
            "Month: {} ({:.0}% of plan)",
            money(snap.month.cost),
            snap.month.cost / plan_usd * 100.0
        );
    } else {
        println!("Month: {}", money(snap.month.cost));
    }
    println!("Today: {}", money(snap.today.cost));
    if let Some(all_time) = snap.all_time {
        println!("All-time: {}", money(all_time.cost));
    }
    println!(
        "Usage: {} tokens, {} calls, {} sessions",
        compact(snap.month.usage.total),
        snap.month.calls,
        snap.month.sessions
    );
    if !snap.unknown_models.is_empty() {
        println!("Unknown price models: {}", snap.unknown_models.join(", "));
    }
    println!("Limits: {}", plan.limits_en);
    println!("{CREDIT_RATES}");
    if let Some(error) = snap.error {
        println!("Error: {error}");
    }
}

fn scan_month(config: &Config) -> Result<Snapshot, String> {
    let home = codex_home();
    let service_tier = configured_service_tier(&home);
    let now = local_time();
    let today = date_from_system(now);
    let cycle_start = current_cycle_start(today, config.cycle_day);
    let cycle_next = next_cycle_start(today, config.cycle_day);
    let metadata = load_metadata(&home);
    let mut snap = Snapshot {
        updated: format!("{:02}:{:02}", now.wHour, now.wMinute),
        codex_home: home,
        service_tier,
        config: config.clone(),
        cycle_start,
        cycle_next,
        cycle_days_left: days_between(today, cycle_next).max(0),
        ..Snapshot::default()
    };

    let mut unknown = HashSet::new();
    for month_dir in cycle_month_dirs(&snap.codex_home, cycle_start, today) {
        if !month_dir.exists() {
            continue;
        }
        for path in jsonl_files(&month_dir) {
            let Some(file_date) = file_date(&path) else {
                continue;
            };
            if file_date < cycle_start || file_date > today {
                continue;
            }

            let key = path_key(&path);
            let model = metadata
                .get(&key)
                .cloned()
                .or_else(|| env::var("CODEX_SAVINGS_MODEL").ok())
                .unwrap_or_else(|| {
                    snap.assumed_models += 1;
                    "gpt-5.5".to_string()
                });

            if let Some(session) = parse_session(&path, &model, service_tier, &mut unknown) {
                snap.month.add(session.clone());
                if file_date == today {
                    snap.today.add(session);
                }
            }
        }
    }
    snap.unknown_models = unknown.into_iter().collect();
    snap.unknown_models.sort();
    Ok(snap)
}

fn calculate_all_time(snap: &mut Snapshot) {
    let home = snap.codex_home.clone();
    let service_tier = snap.service_tier;
    let metadata = load_metadata(&home);
    let mut period = Period::default();
    let mut unknown = HashSet::new();

    for path in jsonl_files(&home.join("sessions")) {
        let key = path_key(&path);
        let model = metadata
            .get(&key)
            .cloned()
            .or_else(|| env::var("CODEX_SAVINGS_MODEL").ok())
            .unwrap_or_else(|| {
                snap.assumed_models += 1;
                "gpt-5.5".to_string()
            });
        if let Some(session) = parse_session(&path, &model, service_tier, &mut unknown) {
            period.add(session);
        }
    }

    snap.all_time = Some(period);
    snap.all_time_updated = {
        let now = local_time();
        format!("{:02}:{:02}", now.wHour, now.wMinute)
    };
    for model in unknown {
        if !snap.unknown_models.contains(&model) {
            snap.unknown_models.push(model);
        }
    }
    snap.unknown_models.sort();
}

fn codex_home() -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn configured_service_tier(codex_home: &Path) -> ServiceTier {
    env::var("CODEX_SAVINGS_SERVICE_TIER")
        .ok()
        .and_then(|value| ServiceTier::from_str(&value))
        .or_else(|| {
            fs::read_to_string(codex_home.join("config.toml"))
                .ok()
                .and_then(|contents| service_tier_from_toml(&contents))
        })
        .unwrap_or_default()
}

fn service_tier_from_toml(contents: &str) -> Option<ServiceTier> {
    contents.lines().find_map(|line| {
        let line = line.split('#').next().unwrap_or("").trim();
        let (key, value) = line.split_once('=')?;
        (key.trim() == "service_tier")
            .then(|| ServiceTier::from_str(value))
            .flatten()
    })
}

fn service_tier_label(tier: ServiceTier) -> &'static str {
    match tier {
        ServiceTier::Standard => "standard",
        ServiceTier::Fast => "fast",
    }
}

fn load_metadata(codex_home: &Path) -> HashMap<String, String> {
    let db = codex_home.join("state_5.sqlite");
    let mut models = HashMap::new();
    let Ok(conn) = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return models;
    };
    let Ok(mut stmt) =
        conn.prepare("select rollout_path, model from threads where rollout_path is not null")
    else {
        return models;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let path: String = row.get(0)?;
        let model: Option<String> = row.get(1)?;
        Ok((path, model.unwrap_or_default()))
    }) else {
        return models;
    };
    for row in rows.flatten() {
        if !row.1.is_empty() {
            models.insert(path_key(Path::new(&row.0)), row.1);
        }
    }
    models
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    visit_jsonl(root, &mut files);
    files.sort();
    files
}

fn visit_jsonl(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_jsonl(&path, files);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
        {
            files.push(path);
        }
    }
}

fn parse_session(
    path: &Path,
    model: &str,
    default_service_tier: ServiceTier,
    unknown: &mut HashSet<String>,
) -> Option<Period> {
    let file = File::open(path).ok()?;
    let mut previous = Usage::default();
    let mut period = Period {
        sessions: 1,
        ..Period::default()
    };

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"total_token_usage\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(usage) = usage_from_event(&value) else {
            continue;
        };
        let delta = match usage.total.cmp(&previous.total) {
            CmpOrdering::Less => usage,
            _ => usage.delta(previous),
        };
        let service_tier = service_tier_from_event(&value).unwrap_or(default_service_tier);
        previous = usage;
        if !delta.any() {
            continue;
        }
        period.usage.add(delta);
        period.cost += cost(delta, model, service_tier, unknown);
        period.calls += 1;
    }

    (period.calls > 0).then_some(period)
}

fn usage_from_json(value: &Value) -> Option<Usage> {
    Some(Usage {
        input: value.get("input_tokens")?.as_u64().unwrap_or(0),
        cached: value
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output: value
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        reasoning: value
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total: value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

fn usage_from_event(value: &Value) -> Option<Usage> {
    [
        "/payload/info/total_token_usage",
        "/payload/total_token_usage",
        "/info/total_token_usage",
        "/total_token_usage",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(usage_from_json))
}

fn service_tier_from_event(value: &Value) -> Option<ServiceTier> {
    [
        "/payload/info/service_tier",
        "/payload/service_tier",
        "/info/service_tier",
        "/service_tier",
        "/payload/info/speed",
        "/payload/speed",
        "/info/speed",
        "/speed",
    ]
    .iter()
    .find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(ServiceTier::from_str)
    })
}

fn cost(
    usage: Usage,
    model: &str,
    service_tier: ServiceTier,
    unknown: &mut HashSet<String>,
) -> f64 {
    let Some((price_prefix, mut price)) = price_entry_for_model(model) else {
        unknown.insert(model.to_string());
        return 0.0;
    };
    if service_tier == ServiceTier::Fast {
        if let Some(multiplier) = fast_multiplier_for_model_prefix(price_prefix) {
            price.input *= multiplier;
            price.cached *= multiplier;
            price.output *= multiplier;
        }
    }
    let uncached = usage.input.saturating_sub(usage.cached);
    (uncached as f64 * price.input
        + usage.cached as f64 * price.cached
        + usage.output as f64 * price.output)
        / 1_000_000.0
}

#[cfg(test)]
fn price_for_model(model: &str) -> Option<Price> {
    price_entry_for_model(model).map(|(_, price)| price)
}

fn price_entry_for_model(model: &str) -> Option<(&'static str, Price)> {
    PRICES
        .iter()
        .filter(|(prefix, _)| model.starts_with(*prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(prefix, price)| (*prefix, *price))
}

fn fast_multiplier_for_model_prefix(prefix: &str) -> Option<f64> {
    match prefix {
        "gpt-5.5" => Some(2.5),
        "gpt-5.4" => Some(2.0),
        _ => None,
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

fn file_date(path: &Path) -> Option<DateKey> {
    let day = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u16>().ok())?;
    let month = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u16>().ok())?;
    let year = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u16>().ok())?;
    Some(DateKey { year, month, day })
}

fn date_from_system(time: SYSTEMTIME) -> DateKey {
    DateKey {
        year: time.wYear,
        month: time.wMonth,
        day: time.wDay,
    }
}

fn current_cycle_start(today: DateKey, cycle_day: u16) -> DateKey {
    let cycle_day = cycle_day.clamp(1, 31);
    let this_start = DateKey {
        year: today.year,
        month: today.month,
        day: cycle_day.min(last_day_of_month(today.year, today.month)),
    };
    if today >= this_start {
        this_start
    } else {
        let (year, month) = previous_month(today.year, today.month);
        DateKey {
            year,
            month,
            day: cycle_day.min(last_day_of_month(year, month)),
        }
    }
}

fn next_cycle_start(today: DateKey, cycle_day: u16) -> DateKey {
    let cycle_day = cycle_day.clamp(1, 31);
    let start = current_cycle_start(today, cycle_day);
    let (year, month) = next_month(start.year, start.month);
    DateKey {
        year,
        month,
        day: cycle_day.min(last_day_of_month(year, month)),
    }
}

fn cycle_month_dirs(home: &Path, start: DateKey, end: DateKey) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let (mut year, mut month) = (start.year, start.month);
    for _ in 0..14 {
        dirs.push(
            home.join("sessions")
                .join(format!("{year:04}"))
                .join(format!("{month:02}")),
        );
        if year == end.year && month == end.month {
            break;
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    dirs
}

fn next_month(year: u16, month: u16) -> (u16, u16) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

fn previous_month(year: u16, month: u16) -> (u16, u16) {
    if month == 1 {
        (year.saturating_sub(1), 12)
    } else {
        (year, month - 1)
    }
}

fn last_day_of_month(year: u16, month: u16) -> u16 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_between(start: DateKey, end: DateKey) -> i32 {
    ordinal_day(end) - ordinal_day(start)
}

fn ordinal_day(date: DateKey) -> i32 {
    let year = date.year as i32;
    let years_before = year - 1;
    let leap_days = years_before / 4 - years_before / 100 + years_before / 400;
    let mut days = years_before * 365 + leap_days;
    let mut month = 1u16;
    while month < date.month {
        days += last_day_of_month(date.year, month) as i32;
        month += 1;
    }
    days + date.day as i32
}

fn format_date(date: DateKey) -> String {
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

fn cycle_days_header_text(lang: Lang, days_left: i32) -> String {
    if days_left == 1 {
        t(lang, "1 day left", "1 día restante").to_string()
    } else {
        format!(
            "{} {}",
            days_left.max(0),
            t(lang, "days left", "días restantes")
        )
    }
}

fn local_time() -> SYSTEMTIME {
    unsafe {
        let mut now = zeroed();
        GetLocalTime(&mut now);
        now
    }
}

unsafe fn run_tray() {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let gdiplus_token = gdiplus_startup();
    SNAPSHOT.set(Mutex::new(Snapshot::default())).ok();
    let instance = GetModuleHandleW(null());
    let class = wide("CodexSavingsTray");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: app_icon(),
        style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
        ..zeroed()
    };
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class.as_ptr(),
        wide("Codex Savings").as_ptr(),
        WS_POPUP,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        POPUP_W,
        POPUP_H,
        null_mut(),
        null_mut(),
        instance,
        null_mut(),
    );
    if hwnd.is_null() {
        gdiplus_shutdown(gdiplus_token);
        return;
    }

    apply_window_visuals(hwnd, POPUP_W, POPUP_H);
    refresh(hwnd);
    tray_icon(hwnd, NIM_ADD);
    SetTimer(hwnd, TIMER_UID, 300_000, None);

    let mut msg = zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    gdiplus_shutdown(gdiplus_token);
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            match lparam as u32 {
                WM_LBUTTONUP => toggle_popup(hwnd),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            if (ID_PLAN_BASE..ID_PLAN_BASE + PLANS.len()).contains(&id) {
                set_plan(PLANS[id - ID_PLAN_BASE].id);
                refresh(hwnd);
            } else {
                match id {
                    ID_REFRESH => refresh(hwnd),
                    ID_ALL_TIME => refresh_all_time(hwnd),
                    ID_CYCLE_DAY => show_cycle_dialog(hwnd),
                    ID_OPEN_CONFIG => open_config(hwnd),
                    ID_OPEN_USAGE => open_url(hwnd, USAGE_URL),
                    ID_EXIT => {
                        DestroyWindow(hwnd);
                    }
                    _ => {}
                }
            }
            0
        }
        WM_TIMER => {
            refresh(hwnd);
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_DPICHANGED => {
            let rect = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            apply_window_visuals(hwnd, rect.right - rect.left, rect.bottom - rect.top);
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            apply_window_visuals(hwnd, scale(hwnd, POPUP_W), scale(hwnd, POPUP_H));
            tray_icon(hwnd, NIM_MODIFY);
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xffff) == WA_INACTIVE as usize {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_UID);
            tray_icon(hwnd, NIM_DELETE);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn refresh(hwnd: HWND) {
    let old_all_time = SNAPSHOT.get().and_then(|s| {
        s.lock()
            .ok()
            .map(|s| (s.all_time.clone(), s.all_time_updated.clone()))
    });
    let config = load_config();
    let mut snap = scan_month(&config).unwrap_or_else(|error| Snapshot {
        error: Some(error),
        config: config.clone(),
        ..Snapshot::default()
    });
    if let Some((all_time, updated)) = old_all_time {
        snap.all_time = all_time;
        snap.all_time_updated = updated;
    }
    if let Some(lock) = SNAPSHOT.get() {
        *lock.lock().unwrap() = snap;
    }
    tray_icon(hwnd, NIM_MODIFY);
    InvalidateRect(hwnd, null(), 1);
}

unsafe fn refresh_all_time(hwnd: HWND) {
    if let Some(lock) = SNAPSHOT.get() {
        let mut snap = lock.lock().unwrap().clone();
        calculate_all_time(&mut snap);
        *lock.lock().unwrap() = snap;
    }
    tray_icon(hwnd, NIM_MODIFY);
    InvalidateRect(hwnd, null(), 1);
}

unsafe fn tray_icon(hwnd: HWND, action: u32) {
    let snap = SNAPSHOT
        .get()
        .and_then(|s| s.lock().ok().map(|s| s.clone()));
    let mut data: NOTIFYICONDATAW = zeroed();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_UID;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAYICON;
    let themed_icon = themed_tray_icon(system_theme().dark);
    let destroy_icon = themed_icon.is_some();
    data.hIcon = themed_icon.unwrap_or_else(|| app_icon());

    let tip = snap
        .as_ref()
        .map(|s| {
            let lang = current_lang(&s.config);
            let plan = current_plan(&s.config);
            let plan_usd = plan_usd(&s.config);
            let mut tip = format!(
                "Codex {}: {} {}",
                plan_name(plan, lang),
                money(s.month.cost),
                t(lang, "this cycle", "este ciclo")
            );
            if plan_usd > 0.0 {
                tip.push_str(&format!(
                    " ({:.0}% {})",
                    s.month.cost / plan_usd * 100.0,
                    plan_name(plan, lang)
                ));
            }
            if let Some(all_time) = &s.all_time {
                tip.push_str(&format!("; total {}", money(all_time.cost)));
            }
            truncate_chars(&tip, 126)
        })
        .unwrap_or_else(|| "Codex savings".to_string());
    copy_wide(&tip, &mut data.szTip);
    Shell_NotifyIconW(action, &data);
    if destroy_icon {
        DestroyIcon(data.hIcon);
    }
}

unsafe fn toggle_popup(hwnd: HWND) {
    if IsWindowVisible(hwnd) != 0 {
        ShowWindow(hwnd, SW_HIDE);
        return;
    }
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..zeroed()
    };
    GetMonitorInfoW(monitor, &mut info);
    let w = scale(hwnd, POPUP_W);
    let h = scale(hwnd, POPUP_H);
    let x = (pt.x - w + scale(hwnd, 20)).clamp(info.rcWork.left, info.rcWork.right - w);
    let y = anchored_popup_y(
        pt.y,
        h,
        info.rcWork.top,
        info.rcWork.bottom,
        scale(hwnd, POPUP_GAP),
    );
    apply_window_visuals(hwnd, w, h);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    SetForegroundWindow(hwnd);
}

unsafe fn apply_window_visuals(hwnd: HWND, w: i32, h: i32) {
    let theme = system_theme();
    let dark = i32::from(theme.dark);
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
        &dark as *const _ as *const c_void,
        size_of::<i32>() as u32,
    );

    let border = DWM_COLOR_NONE;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR as u32,
        &border as *const _ as *const c_void,
        size_of::<COLORREF>() as u32,
    );

    let corner = DWM_WINDOW_CORNER_DONOTROUND;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        &corner as *const _ as *const c_void,
        size_of::<i32>() as u32,
    );

    let backdrop = DWMSBT_NONE;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE as u32,
        &backdrop as *const _ as *const c_void,
        size_of::<i32>() as u32,
    ) >= 0;
    shape_window_region(hwnd, w, h);
}

unsafe fn shape_window_region(hwnd: HWND, w: i32, h: i32) {
    let radius = scale(hwnd, WINDOW_RADIUS * 2);
    let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius, radius);
    if SetWindowRgn(hwnd, region, 1) == 0 {
        DeleteObject(region);
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let config = SNAPSHOT
        .get()
        .and_then(|s| s.lock().ok().map(|s| s.config.clone()))
        .unwrap_or_else(load_config);
    let lang = current_lang(&config);
    let menu = CreatePopupMenu();
    let plans = CreatePopupMenu();
    for (index, plan) in PLANS.iter().enumerate() {
        let checked = if plan.id == config.plan {
            MF_CHECKED
        } else {
            0
        };
        AppendMenuW(
            plans,
            MF_STRING | checked,
            ID_PLAN_BASE + index,
            wide(&plan_menu_label(plan, lang)).as_ptr(),
        );
    }
    AppendMenuW(
        menu,
        MF_POPUP,
        plans as usize,
        wide(t(lang, "Plan", "Plan")).as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_REFRESH,
        wide(t(lang, "Reload", "Recargar")).as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_CYCLE_DAY,
        wide(&format!(
            "{} {}",
            t(lang, "Plan start day", "Día de inicio"),
            config.cycle_day
        ))
        .as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_ALL_TIME,
        wide(t(lang, "Calculate total saved", "Calcular ahorro total")).as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_OPEN_CONFIG,
        wide(t(lang, "Open config", "Abrir config")).as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        ID_OPEN_USAGE,
        wide(t(lang, "Open usage dashboard", "Abrir panel de uso")).as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(
        menu,
        MF_STRING,
        ID_EXIT,
        wide(t(lang, "Exit", "Salir")).as_ptr(),
    );
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    SetForegroundWindow(hwnd);
    TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null());
    DestroyMenu(menu);
}

unsafe fn show_cycle_dialog(parent: HWND) {
    let instance = GetModuleHandleW(null());
    let class = wide("CodexSavingsCycleDialog");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(cycle_dialog_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hIcon: app_icon(),
        style: CS_DROPSHADOW,
        ..zeroed()
    };
    RegisterClassW(&wc);

    let dpi = dpi(parent);
    let w = z(dpi, DIALOG_W);
    let h = z(dpi, DIALOG_H);
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..zeroed()
    };
    GetMonitorInfoW(monitor, &mut info);
    let x = (pt.x - w / 2).clamp(info.rcWork.left, info.rcWork.right - w);
    let y = anchored_popup_y(
        pt.y,
        h,
        info.rcWork.top,
        info.rcWork.bottom,
        z(dpi, POPUP_GAP),
    );

    let config = load_config();
    CYCLE_DIALOG_DAY.store(config.cycle_day.clamp(1, 31), Ordering::Relaxed);
    let lang = current_lang(&config);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class.as_ptr(),
        wide(t(lang, "Plan start date", "Fecha de inicio del plan")).as_ptr(),
        WS_POPUP,
        x,
        y,
        w,
        h,
        parent,
        null_mut(),
        instance,
        null(),
    );
    if hwnd.is_null() {
        return;
    }
    apply_window_visuals(hwnd, w, h);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
}

unsafe extern "system" fn cycle_dialog_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_cycle_dialog(hwnd);
            0
        }
        WM_LBUTTONUP => {
            let dpi = dpi(hwnd);
            let x = unscale(dpi, (lparam & 0xffff) as i16 as i32);
            let y = unscale(dpi, ((lparam >> 16) & 0xffff) as i16 as i32);
            if point_in_rect(x, y, [292, 16, 22, 22]) {
                DestroyWindow(hwnd);
                return 0;
            }
            if point_in_rect(x, y, [158, 340, 74, 30]) {
                save_cycle_dialog(hwnd);
                return 0;
            }
            if point_in_rect(x, y, [240, 340, 74, 30]) {
                DestroyWindow(hwnd);
                return 0;
            }
            if let Some(day) = cycle_dialog_day_at(x, y) {
                CYCLE_DIALOG_DAY.store(day, Ordering::Relaxed);
                InvalidateRect(hwnd, null(), 1);
            }
            0
        }
        WM_KEYDOWN => {
            match wparam as u32 {
                VK_ESCAPE_CODE => {
                    DestroyWindow(hwnd);
                }
                VK_RETURN_CODE => save_cycle_dialog(hwnd),
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            }
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            apply_window_visuals(hwnd, scale(hwnd, DIALOG_W), scale(hwnd, DIALOG_H));
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn save_cycle_dialog(hwnd: HWND) {
    let day = CYCLE_DIALOG_DAY.load(Ordering::Relaxed).clamp(1, 31);
    set_cycle_day(day);
    let parent = GetWindow(hwnd, GW_OWNER);
    if !parent.is_null() {
        refresh(parent);
    }
    DestroyWindow(hwnd);
}

fn cycle_dialog_day_at(x: i32, y: i32) -> Option<u16> {
    let start_x = 22;
    let start_y = 144;
    let cell_w = 38;
    let cell_h = 30;
    let gap = 4;
    for day in 1..=31 {
        let index = day - 1;
        let col = index % 7;
        let row = index / 7;
        let rect = [
            start_x + col * (cell_w + gap),
            start_y + row * (cell_h + gap),
            cell_w,
            cell_h,
        ];
        if point_in_rect(x, y, rect) {
            return Some(day as u16);
        }
    }
    None
}

unsafe fn paint_cycle_dialog(hwnd: HWND) {
    let config = load_config();
    let lang = current_lang(&config);
    let theme = system_theme();
    let selected = CYCLE_DIALOG_DAY.load(Ordering::Relaxed).clamp(1, 31);
    let today = date_from_system(local_time());
    let next_reset = next_cycle_start(today, selected);
    let dpi = dpi(hwnd);
    let mut ps = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let ui = Ui { hdc, dpi };
    let mut rect = zeroed();
    GetClientRect(hwnd, &mut rect);

    fill(hdc, rect, solid_from_argb(theme.bg));
    round_frame_argb(
        ui,
        [1, 1, DIALOG_W - 2, DIALOG_H - 2],
        WINDOW_RADIUS,
        theme.border_argb,
    );
    SetBkMode(hdc, TRANSPARENT as i32);
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));

    draw_font(
        ui,
        t(lang, "Plan start date", "Fecha de inicio"),
        [22, 18, 224, 24],
        theme.text,
        18,
        700,
    );
    draw(
        ui,
        t(
            lang,
            "Choose the recurring monthly anchor.",
            "Elige el día recurrente de cada mes.",
        ),
        [22, 45, 270, 20],
        theme.muted,
    );

    dialog_close(ui, theme);
    card(ui, theme, [20, 78, 290, 50], true);
    draw_font(
        ui,
        &format!("{} {}", t(lang, "Day", "Día"), selected),
        [36, 88, 78, 24],
        theme.text,
        18,
        700,
    );
    draw(
        ui,
        t(lang, "Repeats monthly", "Se repite cada mes"),
        [118, 91, 172, 18],
        theme.muted,
    );
    draw(
        ui,
        &format!(
            "{} {}",
            t(lang, "Next reset", "Próximo reinicio"),
            format_date(next_reset)
        ),
        [118, 108, 172, 18],
        theme.subtle,
    );

    draw(
        ui,
        t(lang, "Start day", "Día de inicio"),
        [24, 132, 120, 18],
        theme.subtle,
    );
    for day in 1..=31 {
        dialog_day_cell(ui, theme, day as u16, selected);
    }

    draw(
        ui,
        t(
            lang,
            "Short months use their last day.",
            "Meses cortos usan su último día.",
        ),
        [24, 316, 284, 18],
        theme.subtle,
    );
    dialog_button(
        ui,
        theme,
        [158, 340, 74, 30],
        t(lang, "Save", "Guardar"),
        true,
    );
    dialog_button(
        ui,
        theme,
        [240, 340, 74, 30],
        t(lang, "Cancel", "Cancelar"),
        false,
    );

    EndPaint(hwnd, &ps);
}

unsafe fn dialog_close(ui: Ui, theme: Theme) {
    round_fill_argb(ui, [292, 16, 22, 22], CONTROL_RADIUS, theme.card_alt);
    draw_center(ui, "x", [292, 18, 22, 18], theme.muted);
}

unsafe fn dialog_day_cell(ui: Ui, theme: Theme, day: u16, selected: u16) {
    let index = day as i32 - 1;
    let col = index % 7;
    let row = index / 7;
    let rect = [22 + col * 42, 144 + row * 34, 38, 30];
    if day == selected {
        round_fill_argb(ui, rect, CONTROL_RADIUS, theme.accent_argb);
        draw_font_center(
            ui,
            &day.to_string(),
            [rect[0], rect[1] + 6, rect[2], 18],
            rgb(255, 255, 255),
            13,
            700,
        );
    } else {
        round_fill_argb(ui, rect, CONTROL_RADIUS, theme.card);
        round_frame_argb(ui, rect, CONTROL_RADIUS, theme.border_argb);
        draw_font_center(
            ui,
            &day.to_string(),
            [rect[0], rect[1] + 6, rect[2], 18],
            theme.text,
            13,
            600,
        );
    }
}

unsafe fn dialog_button(ui: Ui, theme: Theme, rect: [i32; 4], label: &str, primary: bool) {
    if primary {
        round_fill_argb(ui, rect, CONTROL_RADIUS, theme.accent_argb);
        draw_font_center(
            ui,
            label,
            [rect[0], rect[1] + 7, rect[2], 18],
            rgb(255, 255, 255),
            12,
            700,
        );
    } else {
        round_fill_argb(ui, rect, CONTROL_RADIUS, theme.card_alt);
        round_frame_argb(ui, rect, CONTROL_RADIUS, theme.border_argb);
        draw_font_center(
            ui,
            label,
            [rect[0], rect[1] + 7, rect[2], 18],
            theme.text,
            12,
            600,
        );
    }
}

fn point_in_rect(x: i32, y: i32, rect: [i32; 4]) -> bool {
    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3]
}

fn unscale(dpi: i32, value: i32) -> i32 {
    (value * 96 + dpi / 2) / dpi
}

unsafe fn paint(hwnd: HWND) {
    let snap = SNAPSHOT
        .get()
        .and_then(|s| s.lock().ok().map(|s| s.clone()))
        .unwrap_or_default();
    let dpi = dpi(hwnd);
    let lang = current_lang(&snap.config);
    let plan = current_plan(&snap.config);
    let plan_usd = plan_usd(&snap.config);
    let theme = system_theme();
    let mut ps = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let ui = Ui { hdc, dpi };
    let mut rect = zeroed();
    GetClientRect(hwnd, &mut rect);

    fill(hdc, rect, solid_from_argb(theme.bg));
    round_frame_argb(
        ui,
        [1, 1, POPUP_W - 2, POPUP_H - 2],
        WINDOW_RADIUS,
        theme.border_argb,
    );
    SetBkMode(hdc, TRANSPARENT as i32);
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));

    draw_font(
        ui,
        plan_name(plan, lang),
        [CONTENT_PAD, CONTENT_PAD, 210, 30],
        theme.text,
        20,
        700,
    );
    draw_right(
        ui,
        &cycle_days_header_text(lang, snap.cycle_days_left),
        [218, CONTENT_PAD + 7, 150, 18],
        theme.subtle,
    );

    let pct = if plan_usd > 0.0 {
        snap.month.cost / plan_usd
    } else {
        0.0
    };
    let pct_label = if plan_usd > 0.0 && pct > 1.0 {
        format!("+{:.0}%", (pct - 1.0) * 100.0)
    } else if plan_usd > 0.0 {
        format!("{:.0}%", pct * 100.0)
    } else {
        "--".to_string()
    };

    ring(ui, [30, 72, 114, 114], pct, theme);
    let pct_font = if pct_label.chars().count() > 4 {
        24
    } else {
        30
    };
    draw_font_center(ui, &pct_label, [44, 106, 86, 34], theme.text, pct_font, 650);
    draw_center(ui, t(lang, "used", "usado"), [54, 140, 66, 20], theme.muted);

    let month_cost = money(snap.month.cost);
    draw_font(ui, &month_cost, [170, 78, 116, 34], theme.text, 24, 700);
    if plan_usd > 0.0 {
        let limit = format!("/ {}", money(plan_usd));
        let month_w = font_text_width(ui, &month_cost, 24, 700);
        let limit_w = font_text_width(ui, &limit, 12, 400);
        let limit_x = (170 + month_w + 8).min(368 - limit_w);
        draw_font(
            ui,
            &limit,
            [limit_x, 92, 368 - limit_x, 18],
            theme.muted,
            12,
            400,
        );
    } else {
        draw(ui, plan_limit(plan, lang), [170, 114, 190, 20], theme.muted);
    }
    draw(
        ui,
        &format!("{} {}", t(lang, "today", "hoy"), money(snap.today.cost)),
        [170, 120, 190, 20],
        theme.text,
    );
    if plan_usd > 0.0 && snap.month.cost > plan_usd {
        draw(
            ui,
            &format!(
                "{} {}",
                t(lang, "over", "exceso"),
                money(snap.month.cost - plan_usd)
            ),
            [170, 142, 190, 18],
            theme.danger,
        );
    } else if plan_usd > 0.0 {
        let remaining = (plan_usd - snap.month.cost).max(0.0);
        draw(
            ui,
            &format!("{} {}", money(remaining), t(lang, "remaining", "restante")),
            [170, 142, 190, 18],
            theme.subtle,
        );
    }

    let footer_y = POPUP_H - CONTENT_PAD - 18;
    if let Some(error) = &snap.error {
        draw(ui, error, [CONTENT_PAD, footer_y, 210, 18], theme.danger);
    } else {
        draw_right(
            ui,
            &format!("{} {}", t(lang, "updated", "actualizado"), snap.updated),
            [214, footer_y, 154, 18],
            theme.subtle,
        );
    }

    EndPaint(hwnd, &ps);
}

unsafe fn fill(hdc: HDC, rect: RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, &rect, brush);
    DeleteObject(brush);
}

unsafe fn card(ui: Ui, theme: Theme, rect: [i32; 4], alt: bool) {
    let fill = if alt { theme.card_alt } else { theme.card };
    if !round_fill_argb(ui, rect, CONTROL_RADIUS, fill) {
        round_fill(ui, rect, CONTROL_RADIUS, solid_from_argb(fill));
    }
    round_frame_argb(ui, rect, CONTROL_RADIUS, theme.border_argb);
}

unsafe fn ring(ui: Ui, rect: [i32; 4], pct: f64, theme: Theme) {
    draw_arc(ui, rect, 12, 0.0, 360.0, colorref_to_argb(255, theme.track));
    let sweep = 360.0 * pct.clamp(0.0, 1.0) as f32;
    if sweep > 0.0 {
        let color = if pct > 1.0 {
            colorref_to_argb(255, theme.danger)
        } else {
            theme.accent_argb
        };
        draw_arc(ui, rect, 12, -90.0, sweep, color);
    }
}

unsafe fn draw_arc(ui: Ui, rect: [i32; 4], width: i32, start: f32, sweep: f32, color: u32) {
    with_gdiplus(ui.hdc, |graphics| {
        let [x, y, w, h] = rect;
        let mut pen: *mut GpPen = null_mut();
        let ok = GdipCreatePen1(color, zf(ui.dpi, width), UnitPixel, &mut pen) == GdipOk
            && !pen.is_null()
            && GdipSetPenLineCap197819(pen, LineCapRound, LineCapRound, DashCapFlat) == GdipOk
            && GdipDrawArc(
                graphics,
                pen,
                zf(ui.dpi, x),
                zf(ui.dpi, y),
                zf(ui.dpi, w),
                zf(ui.dpi, h),
                start,
                sweep,
            ) == GdipOk;
        if !pen.is_null() {
            GdipDeletePen(pen);
        }
        ok
    });
}

unsafe fn round_fill(ui: Ui, rect: [i32; 4], radius: i32, color: COLORREF) {
    if round_fill_argb(ui, rect, radius, colorref_to_argb(255, color)) {
        return;
    }
    let [x, y, w, h] = rect.map(|v| z(ui.dpi, v));
    let region = round_region(ui, [x, y, w, h], radius);
    let brush = CreateSolidBrush(color);
    FillRgn(ui.hdc, region, brush);
    DeleteObject(brush);
    DeleteObject(region);
}

unsafe fn round_region(ui: Ui, rect: [i32; 4], radius: i32) -> HRGN {
    let [x, y, w, h] = rect;
    let r = z(ui.dpi, radius * 2);
    CreateRoundRectRgn(x, y, x + w, y + h, r, r)
}

unsafe fn round_fill_argb(ui: Ui, rect: [i32; 4], radius: i32, color: u32) -> bool {
    with_gdiplus(ui.hdc, |graphics| {
        let Some(path) = rounded_path(ui.dpi, rect, radius, false) else {
            return false;
        };
        let mut brush: *mut GpSolidFill = null_mut();
        let ok = GdipCreateSolidFill(color, &mut brush) == GdipOk
            && !brush.is_null()
            && GdipFillPath(graphics, brush as *mut GpBrush, path) == GdipOk;
        if !brush.is_null() {
            GdipDeleteBrush(brush as *mut GpBrush);
        }
        GdipDeletePath(path);
        ok
    })
}

unsafe fn round_frame_argb(ui: Ui, rect: [i32; 4], radius: i32, color: u32) -> bool {
    with_gdiplus(ui.hdc, |graphics| {
        let Some(path) = rounded_path(ui.dpi, rect, radius, true) else {
            return false;
        };
        let mut pen: *mut GpPen = null_mut();
        let ok = GdipCreatePen1(color, zf(ui.dpi, 1), UnitPixel, &mut pen) == GdipOk
            && !pen.is_null()
            && GdipDrawPath(graphics, pen, path) == GdipOk;
        if !pen.is_null() {
            GdipDeletePen(pen);
        }
        GdipDeletePath(path);
        ok
    })
}

unsafe fn with_gdiplus<F>(hdc: HDC, draw: F) -> bool
where
    F: FnOnce(*mut GpGraphics) -> bool,
{
    let mut graphics: *mut GpGraphics = null_mut();
    if GdipCreateFromHDC(hdc, &mut graphics) != GdipOk || graphics.is_null() {
        return false;
    }
    GdipSetCompositingMode(graphics, CompositingModeSourceOver);
    GdipSetCompositingQuality(graphics, CompositingQualityHighQuality);
    GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
    GdipSetPixelOffsetMode(graphics, PixelOffsetModeHalf);
    let ok = draw(graphics);
    GdipDeleteGraphics(graphics);
    ok
}

unsafe fn rounded_path(dpi: i32, rect: [i32; 4], radius: i32, stroke: bool) -> Option<*mut GpPath> {
    let [x, y, w, h] = rect;
    if w <= 0 || h <= 0 {
        return None;
    }
    let inset = if stroke { 0.5 } else { 0.0 };
    let x = zf(dpi, x) + inset;
    let y = zf(dpi, y) + inset;
    let w = (zf(dpi, w) - inset * 2.0).max(0.0);
    let h = (zf(dpi, h) - inset * 2.0).max(0.0);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = zf(dpi, radius).min(w / 2.0).min(h / 2.0).max(0.0);
    let d = r * 2.0;

    let mut path: *mut GpPath = null_mut();
    if GdipCreatePath(FillModeWinding, &mut path) != GdipOk || path.is_null() {
        return None;
    }
    if r <= 0.0 {
        let ok = GdipAddPathLine(path, x, y, x + w, y) == GdipOk
            && GdipAddPathLine(path, x + w, y, x + w, y + h) == GdipOk
            && GdipAddPathLine(path, x + w, y + h, x, y + h) == GdipOk
            && GdipAddPathLine(path, x, y + h, x, y) == GdipOk
            && GdipClosePathFigure(path) == GdipOk;
        if ok {
            return Some(path);
        }
        GdipDeletePath(path);
        return None;
    }

    let ok = GdipAddPathArc(path, x, y, d, d, 180.0, 90.0) == GdipOk
        && GdipAddPathLine(path, x + r, y, x + w - r, y) == GdipOk
        && GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0) == GdipOk
        && GdipAddPathLine(path, x + w, y + r, x + w, y + h - r) == GdipOk
        && GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0) == GdipOk
        && GdipAddPathLine(path, x + w - r, y + h, x + r, y + h) == GdipOk
        && GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0) == GdipOk
        && GdipAddPathLine(path, x, y + h - r, x, y + r) == GdipOk
        && GdipClosePathFigure(path) == GdipOk;
    if ok {
        Some(path)
    } else {
        GdipDeletePath(path);
        None
    }
}

unsafe fn draw(ui: Ui, text: &str, rect: [i32; 4], color: COLORREF) {
    let [x, y, w, h] = rect;
    draw_text(
        ui,
        text,
        RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        color,
        DT_LEFT,
    );
}

unsafe fn draw_right(ui: Ui, text: &str, rect: [i32; 4], color: COLORREF) {
    let [x, y, w, h] = rect;
    draw_text(
        ui,
        text,
        RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        color,
        DT_RIGHT,
    );
}

unsafe fn draw_center(ui: Ui, text: &str, rect: [i32; 4], color: COLORREF) {
    let [x, y, w, h] = rect;
    draw_text(
        ui,
        text,
        RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        color,
        DT_CENTER,
    );
}

unsafe fn draw_font(ui: Ui, text: &str, rect: [i32; 4], color: COLORREF, size: i32, weight: i32) {
    draw_font_aligned(ui, text, rect, color, size, weight, DT_LEFT);
}

unsafe fn font_text_width(ui: Ui, text: &str, size: i32, weight: i32) -> i32 {
    let name = wide("Segoe UI");
    let font = CreateFontW(
        -z(ui.dpi, size),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        name.as_ptr(),
    );
    if font.is_null() {
        return 0;
    }
    let old = SelectObject(ui.hdc, font);
    let text = wide(text);
    let mut measured: SIZE = zeroed();
    let ok = GetTextExtentPoint32W(
        ui.hdc,
        text.as_ptr(),
        text.len().saturating_sub(1) as i32,
        &mut measured,
    ) != 0;
    SelectObject(ui.hdc, old);
    DeleteObject(font);
    if ok {
        unscale(ui.dpi, measured.cx)
    } else {
        0
    }
}

unsafe fn draw_font_center(
    ui: Ui,
    text: &str,
    rect: [i32; 4],
    color: COLORREF,
    size: i32,
    weight: i32,
) {
    draw_font_aligned(ui, text, rect, color, size, weight, DT_CENTER);
}

unsafe fn draw_font_aligned(
    ui: Ui,
    text: &str,
    rect: [i32; 4],
    color: COLORREF,
    size: i32,
    weight: i32,
    align: u32,
) {
    let [x, y, w, h] = rect;
    let (hdc, dpi) = (ui.hdc, ui.dpi);
    let name = wide("Segoe UI");
    let font = CreateFontW(
        -z(dpi, size),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        0,
        0,
        0,
        5,
        0,
        name.as_ptr(),
    );
    let old = SelectObject(hdc, font);
    draw_text(
        ui,
        text,
        RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        color,
        align,
    );
    SelectObject(hdc, old);
    DeleteObject(font);
}

unsafe fn draw_text(ui: Ui, text: &str, mut rect: RECT, color: COLORREF, align: u32) {
    rect.left = z(ui.dpi, rect.left);
    rect.top = z(ui.dpi, rect.top);
    rect.right = z(ui.dpi, rect.right);
    rect.bottom = z(ui.dpi, rect.bottom);
    let text = wide(text);
    SetTextColor(ui.hdc, color);
    DrawTextW(
        ui.hdc,
        text.as_ptr(),
        -1,
        &mut rect,
        align | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
}

fn money(value: f64) -> String {
    format!("${value:.2}")
}

fn compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

fn solid_from_argb(color: u32) -> COLORREF {
    rgb(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    )
}

fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn colorref_to_argb(a: u8, color: COLORREF) -> u32 {
    let r = (color & 0xff) as u8;
    let g = ((color >> 8) & 0xff) as u8;
    let b = ((color >> 16) & 0xff) as u8;
    argb(a, r, g, b)
}

fn personalize_dword(name: &str, default: u32) -> u32 {
    unsafe {
        let mut value = default;
        let mut size = size_of::<u32>() as u32;
        let mut ty = 0u32;
        let result = RegGetValueW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            wide(name).as_ptr(),
            RRF_RT_REG_DWORD,
            &mut ty,
            &mut value as *mut _ as *mut c_void,
            &mut size,
        );
        if result == 0 {
            value
        } else {
            default
        }
    }
}

fn windows_apps_dark() -> bool {
    personalize_dword("AppsUseLightTheme", 1) == 0
}

unsafe fn themed_tray_icon(dark: bool) -> Option<HICON> {
    let size = 32i32;
    let source_icon = app_icon_sized(size);
    if source_icon.is_null() {
        return None;
    }

    let mut source: *mut GpBitmap = null_mut();
    if GdipCreateBitmapFromHICON(source_icon, &mut source) != GdipOk || source.is_null() {
        return None;
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        },
        ..zeroed()
    };
    let mut bits: *mut c_void = null_mut();
    let hbm_color = CreateDIBSection(null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if hbm_color.is_null() || bits.is_null() {
        if !hbm_color.is_null() {
            DeleteObject(hbm_color);
        }
        GdipDisposeImage(source as *mut GpImage);
        return None;
    }

    let pixels = slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
    render_tray_icon_from_source(pixels, size, source, dark);
    GdipDisposeImage(source as *mut GpImage);

    let hbm_mask: HBITMAP = CreateBitmap(size, size, 1, 1, null());
    if hbm_mask.is_null() {
        DeleteObject(hbm_color);
        return None;
    }

    let info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };
    let icon = CreateIconIndirect(&info);
    DeleteObject(hbm_color);
    DeleteObject(hbm_mask);
    (!icon.is_null()).then_some(icon)
}

unsafe fn render_tray_icon_from_source(
    pixels: &mut [u32],
    size: i32,
    source: *mut GpBitmap,
    dark: bool,
) {
    pixels.fill(0);
    let foreground = if dark { 0x00ff_ffff } else { 0x0000_0000 };
    for y in 0..size {
        for x in 0..size {
            let mut color = 0u32;
            if GdipBitmapGetPixel(source, x, y, &mut color) != GdipOk {
                continue;
            }
            let idx = (y * size + x) as usize;
            pixels[idx] = extract_tray_stroke(color, foreground);
        }
    }
}

fn extract_tray_stroke(color: u32, foreground: u32) -> u32 {
    let alpha = (color >> 24) & 0xff;
    if alpha == 0 {
        return 0;
    }
    let red = (color >> 16) & 0xff;
    let green = (color >> 8) & 0xff;
    let blue = color & 0xff;
    let luma = (red * 30 + green * 59 + blue * 11) / 100;
    let stroke_alpha = alpha * (255 - luma) / 255;
    if stroke_alpha == 0 {
        0
    } else {
        (stroke_alpha << 24) | foreground
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn copy_wide<const N: usize>(value: &str, target: &mut [u16; N]) {
    let text = wide(value);
    let len = text.len().min(N);
    target[..len].copy_from_slice(&text[..len]);
    if len == N {
        target[N - 1] = 0;
    }
}

fn load_config() -> Config {
    let path = config_path();
    if !path.exists() {
        write_config(&Config::default());
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(config_from_json)
        .unwrap_or_default()
}

fn config_from_json(value: Value) -> Config {
    let default = Config::default();
    let plan = value
        .get("plan")
        .and_then(Value::as_str)
        .filter(|id| plan_by_id(id).is_some())
        .unwrap_or(&default.plan)
        .to_string();
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .filter(|lang| matches!(*lang, "auto" | "en" | "es"))
        .unwrap_or(&default.language)
        .to_string();
    let monthly_usd_override = value
        .get("monthly_usd_override")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);
    let cycle_day = value
        .get("cycle_day")
        .and_then(Value::as_u64)
        .filter(|day| (1..=31).contains(day))
        .map(|day| day as u16)
        .unwrap_or(default.cycle_day);
    Config {
        plan,
        monthly_usd_override,
        language,
        cycle_day,
    }
}

fn write_config(config: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let monthly = config
        .monthly_usd_override
        .and_then(serde_json::Number::from_f64)
        .map(Value::Number)
        .unwrap_or(Value::Null);
    let text = serde_json::json!({
        "plan": config.plan.clone(),
        "monthly_usd_override": monthly,
        "language": config.language.clone(),
        "cycle_day": config.cycle_day,
    });
    let _ = fs::write(
        path,
        serde_json::to_string_pretty(&text).unwrap_or_default(),
    );
}

fn config_path() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Codex Savings Tracker")
        .join("config.json")
}

fn set_plan(id: &str) {
    if plan_by_id(id).is_none() {
        return;
    }
    let mut config = load_config();
    config.plan = id.to_string();
    config.monthly_usd_override = None;
    write_config(&config);
}

fn set_cycle_day(day: u16) {
    let mut config = load_config();
    config.cycle_day = day.clamp(1, 31);
    write_config(&config);
}

fn plan_by_id(id: &str) -> Option<&'static Plan> {
    PLANS.iter().find(|plan| plan.id == id)
}

fn current_plan(config: &Config) -> &'static Plan {
    plan_by_id(&config.plan).unwrap_or(&PLANS[2])
}

fn plan_usd(config: &Config) -> f64 {
    config
        .monthly_usd_override
        .unwrap_or_else(|| current_plan(config).usd)
}

fn current_lang(config: &Config) -> Lang {
    match config.language.as_str() {
        "en" => Lang::En,
        "es" => Lang::Es,
        _ => {
            let primary = unsafe { GetUserDefaultUILanguage() } & 0x03ff;
            if primary == 0x000a {
                Lang::Es
            } else {
                Lang::En
            }
        }
    }
}

fn plan_name(plan: &Plan, lang: Lang) -> &'static str {
    t(lang, plan.en, plan.es)
}

fn plan_limit(plan: &Plan, lang: Lang) -> &'static str {
    t(lang, plan.limits_en, plan.limits_es)
}

fn plan_menu_label(plan: &Plan, lang: Lang) -> String {
    if plan.usd > 0.0 {
        format!("{} ({}/mo)", plan_name(plan, lang), money(plan.usd))
    } else {
        plan_name(plan, lang).to_string()
    }
}

fn t(lang: Lang, en: &'static str, es: &'static str) -> &'static str {
    if lang == Lang::Es {
        es
    } else {
        en
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in value.chars().take(max) {
        out.push(ch);
    }
    out
}

fn z(dpi: i32, value: i32) -> i32 {
    (value * dpi + 48) / 96
}

fn zf(dpi: i32, value: i32) -> f32 {
    value as f32 * dpi as f32 / 96.0
}

fn dpi(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        96
    } else {
        dpi as i32
    }
}

fn scale(hwnd: HWND, value: i32) -> i32 {
    z(dpi(hwnd), value)
}

fn anchored_popup_y(cursor_y: i32, popup_h: i32, work_top: i32, work_bottom: i32, gap: i32) -> i32 {
    let max_y = (work_bottom - popup_h - gap).max(work_top);
    (cursor_y - popup_h - gap).clamp(work_top, max_y)
}

unsafe fn gdiplus_startup() -> Option<usize> {
    let input = GdiplusStartupInput {
        GdiplusVersion: 1,
        DebugEventCallback: 0,
        SuppressBackgroundThread: 0,
        SuppressExternalCodecs: 0,
    };
    let mut token = 0usize;
    if GdiplusStartup(&mut token, &input, null_mut()) == GdipOk {
        Some(token)
    } else {
        None
    }
}

unsafe fn gdiplus_shutdown(token: Option<usize>) {
    if let Some(token) = token {
        GdiplusShutdown(token);
    }
}

unsafe fn app_icon() -> HICON {
    app_icon_sized(0)
}

unsafe fn app_icon_sized(size: i32) -> HICON {
    let flags = if size == 0 {
        LR_DEFAULTSIZE | LR_SHARED
    } else {
        LR_SHARED
    };
    let icon = LoadImageW(
        GetModuleHandleW(null()),
        std::ptr::without_provenance::<u16>(1),
        IMAGE_ICON,
        size,
        size,
        flags,
    ) as HICON;
    if icon.is_null() {
        LoadIconW(null_mut(), IDI_APPLICATION)
    } else {
        icon
    }
}

unsafe fn open_config(hwnd: HWND) {
    let _ = load_config();
    let path = config_path().to_string_lossy().to_string();
    open_url(hwnd, &path);
}

unsafe fn open_url(hwnd: HWND, target: &str) {
    ShellExecuteW(
        hwnd,
        wide("open").as_ptr(),
        wide(target).as_ptr(),
        null(),
        null(),
        SW_SHOWNORMAL,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_jsonl(contents: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("codex-savings-test-{id}.jsonl"));
        let mut file = File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    fn usage_line(input: u64, cached: u64, output: u64, reasoning: u64, total: u64) -> String {
        serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": cached,
                        "output_tokens": output,
                        "reasoning_output_tokens": reasoning,
                        "total_tokens": total,
                    }
                }
            }
        })
        .to_string()
    }

    fn direct_usage_line(
        input: u64,
        cached: u64,
        output: u64,
        reasoning: u64,
        total: u64,
    ) -> String {
        serde_json::json!({
            "type": "token_count",
            "total_token_usage": {
                "input_tokens": input,
                "cached_input_tokens": cached,
                "output_tokens": output,
                "reasoning_output_tokens": reasoning,
                "total_tokens": total,
            }
        })
        .to_string()
    }

    #[test]
    fn price_uses_longest_matching_model_prefix() {
        let price = price_for_model("gpt-5.4-mini-2026-04-28").unwrap();
        assert_eq!(price.input, 0.75);
        assert_eq!(price.output, 4.5);
    }

    #[test]
    fn fast_tier_multiplies_supported_models_only() {
        let usage = Usage {
            input: 1_000_000,
            cached: 0,
            output: 0,
            reasoning: 0,
            total: 1_000_000,
        };
        let mut unknown = HashSet::new();

        assert_eq!(
            cost(usage, "gpt-5.5", ServiceTier::Standard, &mut unknown),
            5.0
        );
        assert_eq!(
            cost(usage, "gpt-5.5", ServiceTier::Fast, &mut unknown),
            12.5
        );
        assert_eq!(cost(usage, "gpt-5.4", ServiceTier::Fast, &mut unknown), 5.0);
        assert_eq!(
            cost(usage, "gpt-5.4-mini", ServiceTier::Fast, &mut unknown),
            0.75
        );
        assert!(unknown.is_empty());
    }

    #[test]
    fn usage_delta_saturates_per_field() {
        let current = Usage {
            input: 12,
            cached: 2,
            output: 5,
            reasoning: 1,
            total: 17,
        };
        let previous = Usage {
            input: 10,
            cached: 3,
            output: 1,
            reasoning: 2,
            total: 14,
        };
        let delta = current.delta(previous);
        assert_eq!(delta.input, 2);
        assert_eq!(delta.cached, 0);
        assert_eq!(delta.output, 4);
        assert_eq!(delta.reasoning, 0);
        assert_eq!(delta.total, 3);
    }

    #[test]
    fn parse_session_dedupes_repeated_cumulative_usage() {
        let contents = [
            usage_line(100, 20, 10, 4, 110),
            usage_line(100, 20, 10, 4, 110),
            usage_line(150, 50, 20, 9, 170),
        ]
        .join("\n");
        let path = temp_jsonl(&contents);
        let mut unknown = HashSet::new();
        let period = parse_session(&path, "gpt-5.5", ServiceTier::Standard, &mut unknown).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.calls, 2);
        assert_eq!(period.sessions, 1);
        assert_eq!(period.usage.input, 150);
        assert_eq!(period.usage.cached, 50);
        assert_eq!(period.usage.output, 20);
        assert_eq!(period.usage.reasoning, 9);
        assert_eq!(period.usage.total, 170);
        assert!(period.cost > 0.0);
        assert!(unknown.is_empty());
    }

    #[test]
    fn parse_session_counts_counter_reset_as_new_total() {
        let contents = [usage_line(100, 10, 10, 2, 110), usage_line(8, 0, 2, 1, 10)].join("\n");
        let path = temp_jsonl(&contents);
        let mut unknown = HashSet::new();
        let period = parse_session(&path, "gpt-5.5", ServiceTier::Standard, &mut unknown).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.calls, 2);
        assert_eq!(period.usage.input, 108);
        assert_eq!(period.usage.output, 12);
        assert_eq!(period.usage.total, 120);
    }

    #[test]
    fn parse_session_accepts_direct_codex_cli_usage_shape() {
        let path = temp_jsonl(&direct_usage_line(40, 5, 10, 3, 50));
        let mut unknown = HashSet::new();
        let period = parse_session(&path, "gpt-5.5", ServiceTier::Standard, &mut unknown).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.calls, 1);
        assert_eq!(period.usage.input, 40);
        assert_eq!(period.usage.cached, 5);
        assert_eq!(period.usage.output, 10);
        assert_eq!(period.usage.total, 50);
    }

    #[test]
    fn parse_session_uses_event_service_tier_before_default() {
        let line = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "service_tier": "fast",
                    "total_token_usage": {
                        "input_tokens": 1_000_000,
                        "cached_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_output_tokens": 0,
                        "total_tokens": 1_000_000,
                    }
                }
            }
        })
        .to_string();
        let path = temp_jsonl(&line);
        let mut unknown = HashSet::new();
        let period = parse_session(&path, "gpt-5.5", ServiceTier::Standard, &mut unknown).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.cost, 12.5);
        assert!(unknown.is_empty());
    }

    #[test]
    fn service_tier_from_toml_reads_fast_setting() {
        let contents = r#"
            model = "gpt-5.5"
            service_tier = "fast"
        "#;
        assert_eq!(service_tier_from_toml(contents), Some(ServiceTier::Fast));
    }

    #[test]
    fn config_parses_plan_override_and_language() {
        let config = config_from_json(serde_json::json!({
            "plan": "pro_20x",
            "monthly_usd_override": 199.0,
            "language": "es",
            "cycle_day": 15
        }));
        assert_eq!(current_plan(&config).id, "pro_20x");
        assert_eq!(plan_usd(&config), 199.0);
        assert_eq!(current_lang(&config), Lang::Es);
        assert_eq!(config.cycle_day, 15);
    }

    #[test]
    fn config_rejects_unknown_values() {
        let config = config_from_json(serde_json::json!({
            "plan": "unknown",
            "monthly_usd_override": -1.0,
            "language": "fr",
            "cycle_day": 40
        }));
        assert_eq!(current_plan(&config).id, "plus");
        assert_eq!(plan_usd(&config), 20.0);
        assert_eq!(config.language, "auto");
        assert_eq!(config.cycle_day, 1);
    }

    #[test]
    fn current_cycle_start_rolls_to_previous_month() {
        let start = current_cycle_start(
            DateKey {
                year: 2026,
                month: 5,
                day: 1,
            },
            15,
        );
        assert_eq!(
            start,
            DateKey {
                year: 2026,
                month: 4,
                day: 15,
            }
        );
    }

    #[test]
    fn current_cycle_start_clamps_long_month_day() {
        let start = current_cycle_start(
            DateKey {
                year: 2026,
                month: 3,
                day: 1,
            },
            31,
        );
        assert_eq!(
            start,
            DateKey {
                year: 2026,
                month: 2,
                day: 28,
            }
        );
    }

    #[test]
    fn next_cycle_start_keeps_monthly_anchor_after_short_month() {
        let next = next_cycle_start(
            DateKey {
                year: 2026,
                month: 2,
                day: 28,
            },
            31,
        );
        assert_eq!(
            next,
            DateKey {
                year: 2026,
                month: 3,
                day: 31,
            }
        );
    }

    #[test]
    fn next_cycle_start_clamps_leap_year_february() {
        let next = next_cycle_start(
            DateKey {
                year: 2028,
                month: 1,
                day: 31,
            },
            31,
        );
        assert_eq!(
            next,
            DateKey {
                year: 2028,
                month: 2,
                day: 29,
            }
        );
    }

    #[test]
    fn days_between_handles_month_boundaries() {
        assert_eq!(
            days_between(
                DateKey {
                    year: 2026,
                    month: 2,
                    day: 28,
                },
                DateKey {
                    year: 2026,
                    month: 3,
                    day: 31,
                }
            ),
            31
        );
    }

    #[test]
    fn official_plan_ranges_are_available_locally() {
        let plus = plan_by_id("plus").unwrap();
        let pro = plan_by_id("pro_20x").unwrap();
        assert!(plus.limits_en.contains("15-80"));
        assert!(pro.limits_en.contains("300-1600"));
        assert!(CREDIT_RATES.contains("125/12.5/750"));
    }

    #[test]
    fn tooltip_truncation_keeps_char_boundary() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
        assert_eq!(truncate_chars("ahorro", 20), "ahorro");
    }

    #[test]
    fn anchored_popup_y_preserves_taskbar_gap() {
        let y = anchored_popup_y(1062, 420, 0, 1032, 12);
        assert_eq!(y + 420, 1020);
    }

    #[test]
    fn anchored_popup_y_clamps_to_work_area_top_when_tight() {
        assert_eq!(anchored_popup_y(40, 420, 20, 300, 12), 20);
    }

    #[test]
    fn all_time_is_on_demand_by_default() {
        assert!(Snapshot::default().all_time.is_none());
    }
}
