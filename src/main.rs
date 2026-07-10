#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::c_void,
    fs::{self, File},
    io::{BufRead, BufReader},
    mem::{size_of, zeroed},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    slice,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    thread,
    time::SystemTime,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, FreeLibrary, GetLastError, COLORREF, ERROR_ALREADY_EXISTS, FILETIME, HANDLE,
        HWND, LPARAM, LRESULT, POINT, RECT, SIZE, SYSTEMTIME, WPARAM,
    },
    Globalization::GetUserDefaultUILanguage,
    Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_NONE, DWMWA_BORDER_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    },
    Graphics::Gdi::{
        BeginPaint, CreateBitmap, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteObject,
        DrawTextW, EndPaint, FillRect, GetMonitorInfoW, GetStockObject, GetTextExtentPoint32W,
        InvalidateRect, MonitorFromPoint, SelectObject, SetBkMode, SetTextColor, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DEFAULT_GUI_FONT, DIB_RGB_COLORS, DT_CENTER, DT_END_ELLIPSIS,
        DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, HBITMAP, HDC, HMONITOR, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, TRANSPARENT,
    },
    Graphics::GdiPlus::{
        CompositingModeSourceOver, CompositingQualityHighQuality, DashCapFlat, GdipBitmapGetPixel,
        GdipCreateBitmapFromHICON, GdipCreateFromHDC, GdipCreatePen1, GdipDeleteGraphics,
        GdipDeletePen, GdipDisposeImage, GdipDrawArc, GdipSetCompositingMode,
        GdipSetCompositingQuality, GdipSetPenLineCap197819, GdipSetPixelOffsetMode,
        GdipSetSmoothingMode, GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, GpBitmap,
        GpGraphics, GpImage, GpPen, LineCapRound, Ok as GdipOk, PixelOffsetModeHalf,
        SmoothingModeAntiAlias, UnitPixel,
    },
    System::{
        LibraryLoader::{
            GetModuleHandleW, GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
        },
        Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
        SystemInformation::GetLocalTime,
        Threading::{CreateMutexW, Sleep},
        Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
    },
    UI::{
        HiDpi::{
            GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
        },
        Shell::{
            ShellExecuteW, Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE,
            NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETFOCUS, NOTIFYICONDATAW,
            NOTIFYICONIDENTIFIER,
        },
        WindowsAndMessaging::*,
    },
};

const WM_TRAYICON: u32 = WM_USER + 7;
const WM_SHOW_POPUP: u32 = WM_APP + 1;
const WM_REFRESHED: u32 = WM_APP + 2;
const WM_ALL_TIME_REFRESHED: u32 = WM_APP + 3;
const TRAY_UID: u32 = 1;
const TIMER_UID: usize = 1;
const HIDE_TIMER_UID: usize = 2;
const ID_REFRESH: usize = 1001;
const ID_EXIT: usize = 1002;
const ID_ALL_TIME: usize = 1003;
const ID_OPEN_CONFIG: usize = 1004;
const ID_OPEN_USAGE: usize = 1005;
const ID_PLAN_BASE: usize = 1100;
const ID_DAY_BASE: usize = 1200;
const ID_LANG_BASE: usize = 1300;
const ID_CUSTOM_MINUS_10: usize = 1400;
const ID_CUSTOM_MINUS_1: usize = 1401;
const ID_CUSTOM_PLUS_1: usize = 1402;
const ID_CUSTOM_PLUS_10: usize = 1403;
const POPUP_W: i32 = 392;
const POPUP_H: i32 = 226;
const POPUP_GAP: i32 = 12;
const CONTENT_PAD: i32 = 24;
const POPUP_ANIMATION_MS: u32 = 160;
const DWM_COLOR_NONE: COLORREF = 0xffff_fffe;
const USAGE_URL: &str = "https://chatgpt.com/codex/cloud/settings/analytics";
const APP_WINDOW_CLASS: &str = "CodexSavingsTray";
const APP_WINDOW_TITLE: &str = "Codex Savings";
const INSTANCE_MUTEX_NAME: &str = "Local\\CodexSavingsTraySingleInstance";

#[derive(Clone, Copy)]
struct Price {
    input: f64,
    cached: f64,
    cache_write: f64,
    output: f64,
    long_context: bool,
}

const fn price(input: f64, cached: f64, output: f64) -> Price {
    Price {
        input,
        cached,
        cache_write: input,
        output,
        long_context: false,
    }
}

const fn contextual_price(input: f64, cached: f64, cache_write: f64, output: f64) -> Price {
    Price {
        input,
        cached,
        cache_write,
        output,
        long_context: true,
    }
}

#[rustfmt::skip]
const PRICES: &[(&str, Price)] = &[
    ("gpt-5.6-sol", contextual_price(5.0, 0.5, 6.25, 30.0)),
    ("gpt-5.6-terra", contextual_price(2.5, 0.25, 3.125, 15.0)),
    ("gpt-5.6-luna", contextual_price(1.0, 0.1, 1.25, 6.0)),
    ("gpt-5.6", contextual_price(5.0, 0.5, 6.25, 30.0)),
    ("gpt-5.5", contextual_price(5.0, 0.5, 5.0, 30.0)),
    ("gpt-5.4-mini", price(0.75, 0.075, 4.5)),
    ("gpt-5.4-nano", price(0.20, 0.02, 1.25)),
    ("gpt-5.4", contextual_price(2.5, 0.25, 2.5, 15.0)),
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
    plan("plus", "Plus", "Plus", 20.0, "5h: Sol 15-90, Terra 20-110, Luna 50-280", "5h: Sol 15-90, Terra 20-110, Luna 50-280"),
    plan("pro_5x", "Pro 5x", "Pro 5x", 100.0, "5h: Sol 75-450, Terra 100-550, Luna 250-1400", "5h: Sol 75-450, Terra 100-550, Luna 250-1400"),
    plan("pro_20x", "Pro 20x", "Pro 20x", 200.0, "5h: Sol 300-1800, Terra 400-2200, Luna 1000-5600", "5h: Sol 300-1800, Terra 400-2200, Luna 1000-5600"),
    plan("business", "Business", "Business", 20.0, "$20/user annual; Plus-like limits", "$20/usuario anual; limites tipo Plus"),
    plan("enterprise_edu", "Enterprise/Edu", "Enterprise/Edu", 0.0, "Credits or Plus-like seats", "Creditos o asientos tipo Plus"),
    plan("api_key", "API Key", "Clave API", 0.0, "Usage-based API pricing", "Precio API por uso"),
    plan("custom", "Custom", "Personalizado", 0.0, "Manual monthly amount", "Monto mensual manual"),
];

const CREDIT_RATES: &str =
    "Credits/1M: Sol 125/12.5/750; Terra 62.5/6.25/375; Luna 25/2.5/150; fast 5.5 x2.5, 5.4 x2";

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
    cache_write: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

impl Usage {
    fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cached += other.cached;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.total += other.total;
    }

    fn delta(self, previous: Usage) -> Usage {
        Usage {
            input: self.input.saturating_sub(previous.input),
            cached: self.cached.saturating_sub(previous.cached),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            output: self.output.saturating_sub(previous.output),
            reasoning: self.reasoning.saturating_sub(previous.reasoning),
            total: self.total.saturating_sub(previous.total),
        }
    }

    fn any(self) -> bool {
        self.input + self.cached + self.cache_write + self.output + self.reasoning > 0
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

struct InstanceGuard {
    handle: HANDLE,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone)]
struct CurrentCacheEntry {
    stamp: FileStamp,
    cycle_start: DateKey,
    today: DateKey,
    fallback_model: String,
    service_tier: ServiceTier,
    cycle: Option<Period>,
    day: Option<Period>,
    unknown_models: Vec<String>,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
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
static CURRENT_CACHE: OnceLock<Mutex<HashMap<PathBuf, CurrentCacheEntry>>> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
static THEMED_TRAY_ICONS: OnceLock<[usize; 2]> = OnceLock::new();
static REFRESHING: AtomicBool = AtomicBool::new(false);
static REFRESH_PENDING: AtomicBool = AtomicBool::new(false);
static ALL_TIME_REFRESHING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Ui {
    hdc: HDC,
    dpi: i32,
}

#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    bg: u32,
    text: COLORREF,
    muted: COLORREF,
    subtle: COLORREF,
    track: COLORREF,
    accent_argb: u32,
    success: COLORREF,
    warning: COLORREF,
    danger: COLORREF,
}

fn system_theme() -> Theme {
    if windows_apps_dark() || windows_system_dark() {
        Theme {
            dark: true,
            bg: argb(255, 32, 32, 32),
            text: rgb(255, 255, 255),
            muted: rgb(211, 211, 211),
            subtle: rgb(158, 158, 158),
            track: rgb(64, 64, 64),
            accent_argb: argb(255, 96, 205, 255),
            success: rgb(108, 203, 95),
            warning: rgb(252, 225, 0),
            danger: rgb(255, 153, 164),
        }
    } else {
        Theme {
            dark: false,
            bg: argb(255, 243, 243, 243),
            text: rgb(26, 26, 26),
            muted: rgb(82, 82, 82),
            subtle: rgb(112, 112, 112),
            track: rgb(221, 221, 221),
            accent_argb: argb(255, 0, 103, 192),
            success: rgb(15, 123, 15),
            warning: rgb(157, 93, 0),
            danger: rgb(196, 43, 28),
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
    let fallback_model =
        env::var("CODEX_SAVINGS_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".to_string());
    for month_dir in cycle_scan_month_dirs(&snap.codex_home, cycle_start, today) {
        if !month_dir.exists() {
            continue;
        }
        for path in jsonl_files(&month_dir) {
            let Some(file_date) = file_date(&path) else {
                continue;
            };
            if file_date > today {
                continue;
            }
            if !session_may_overlap(&path, file_date, cycle_start) {
                continue;
            }

            let key = path_key(&path);
            let model = metadata.get(&key).cloned().unwrap_or_else(|| {
                snap.assumed_models += 1;
                fallback_model.clone()
            });

            let (cycle, day) = cached_session_current(
                &path,
                &model,
                service_tier,
                cycle_start,
                today,
                &mut unknown,
            );
            if let Some(session) = cycle {
                snap.month.add(session);
            }
            if let Some(session) = day {
                snap.today.add(session);
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
                "gpt-5.6-sol".to_string()
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
    let mut tier = None;
    let mut in_features = false;
    let mut fast_mode = false;
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_features = line == "[features]";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "service_tier" => tier = ServiceTier::from_str(value),
            "fast_mode" if in_features => fast_mode = value.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    match tier {
        Some(ServiceTier::Fast) if !fast_mode => Some(ServiceTier::Standard),
        tier => tier,
    }
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
    parse_session_filtered(path, model, default_service_tier, None, unknown)
}

#[cfg(test)]
fn parse_session_between(
    path: &Path,
    model: &str,
    default_service_tier: ServiceTier,
    start: DateKey,
    end: DateKey,
    unknown: &mut HashSet<String>,
) -> Option<Period> {
    parse_session_filtered(
        path,
        model,
        default_service_tier,
        Some((start, end)),
        unknown,
    )
}

fn parse_session_filtered(
    path: &Path,
    model: &str,
    default_service_tier: ServiceTier,
    date_range: Option<(DateKey, DateKey)>,
    unknown: &mut HashSet<String>,
) -> Option<Period> {
    let mut period = Period {
        sessions: 1,
        ..Period::default()
    };
    visit_session_deltas(
        path,
        model,
        default_service_tier,
        date_range.is_some(),
        |delta, date, service_tier, current_model| {
            if let Some((start, end)) = date_range {
                let Some(date) = date else {
                    return;
                };
                if date < start || date > end {
                    return;
                }
            }
            period.usage.add(delta);
            period.cost += cost(delta, current_model, service_tier, unknown);
            period.calls += 1;
        },
    )?;

    (period.calls > 0).then_some(period)
}

fn parse_session_current(
    path: &Path,
    model: &str,
    default_service_tier: ServiceTier,
    cycle_start: DateKey,
    today: DateKey,
    unknown: &mut HashSet<String>,
) -> (Option<Period>, Option<Period>) {
    let mut cycle = Period {
        sessions: 1,
        ..Period::default()
    };
    let mut day = cycle.clone();
    let _ = visit_session_deltas(
        path,
        model,
        default_service_tier,
        true,
        |delta, date, service_tier, current_model| {
            let Some(date) = date else { return };
            let event_cost = cost(delta, current_model, service_tier, unknown);
            if (cycle_start..=today).contains(&date) {
                cycle.usage.add(delta);
                cycle.cost += event_cost;
                cycle.calls += 1;
            }
            if date == today {
                day.usage.add(delta);
                day.cost += event_cost;
                day.calls += 1;
            }
        },
    );
    (
        (cycle.calls > 0).then_some(cycle),
        (day.calls > 0).then_some(day),
    )
}

fn cached_session_current(
    path: &Path,
    model: &str,
    service_tier: ServiceTier,
    cycle_start: DateKey,
    today: DateKey,
    unknown: &mut HashSet<String>,
) -> (Option<Period>, Option<Period>) {
    let Some(stamp) = file_stamp(path) else {
        return parse_session_current(path, model, service_tier, cycle_start, today, unknown);
    };
    let cache = CURRENT_CACHE.get_or_init(Default::default);
    if let Some(entry) = cache.lock().ok().and_then(|cache| cache.get(path).cloned()) {
        if entry.stamp == stamp
            && entry.cycle_start == cycle_start
            && entry.today == today
            && entry.fallback_model == model
            && entry.service_tier == service_tier
        {
            unknown.extend(entry.unknown_models);
            return (entry.cycle, entry.day);
        }
    }

    let mut file_unknown = HashSet::new();
    let (cycle, day) = parse_session_current(
        path,
        model,
        service_tier,
        cycle_start,
        today,
        &mut file_unknown,
    );
    unknown.extend(file_unknown.iter().cloned());
    if let Ok(mut cache) = cache.lock() {
        cache.insert(
            path.to_path_buf(),
            CurrentCacheEntry {
                stamp,
                cycle_start,
                today,
                fallback_model: model.to_string(),
                service_tier,
                cycle: cycle.clone(),
                day: day.clone(),
                unknown_models: file_unknown.into_iter().collect(),
            },
        );
    }
    (cycle, day)
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn session_may_overlap(path: &Path, created: DateKey, cycle_start: DateKey) -> bool {
    created >= cycle_start || modified_local_date(path).is_none_or(|date| date >= cycle_start)
}

fn modified_local_date(path: &Path) -> Option<DateKey> {
    let ticks = fs::metadata(path).ok()?.last_write_time();
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    unsafe {
        let mut utc = zeroed();
        let mut local = zeroed();
        if FileTimeToSystemTime(&file_time, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) == 0
        {
            return None;
        }
        Some(date_from_system(local))
    }
}

fn visit_session_deltas(
    path: &Path,
    model: &str,
    default_service_tier: ServiceTier,
    dated: bool,
    mut visit: impl FnMut(Usage, Option<DateKey>, ServiceTier, &str),
) -> Option<()> {
    const USAGE_MARKER: &str = "\"total_token_usage\"";
    const CONTEXT_MARKER: &str = "\"turn_context\"";
    let file = File::open(path).ok()?;
    let fallback_date = file_date(path);
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut line = Vec::new();
    let mut previous = Usage::default();
    let mut current_model = model.to_string();

    while reader.read_until(b'\n', &mut line).ok()? > 0 {
        let Ok(text) = std::str::from_utf8(&line) else {
            line.clear();
            continue;
        };
        let has_usage = text.contains(USAGE_MARKER);
        let has_context = text.contains(CONTEXT_MARKER);
        if has_usage || has_context {
            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                if has_context {
                    if let Some(model) = model_from_event(&value) {
                        current_model.clear();
                        current_model.push_str(model);
                    }
                } else if let Some(usage) = usage_from_event(&value) {
                    let had_previous = previous.any();
                    let mut delta = if usage.total < previous.total {
                        usage
                    } else {
                        usage.delta(previous)
                    };
                    previous = usage;
                    if dated && !had_previous {
                        delta = last_usage_from_event(&value)
                            .filter(|usage| usage.any())
                            .unwrap_or(delta);
                    }
                    if delta.any() {
                        let date = dated
                            .then(|| event_local_date(&value).or(fallback_date))
                            .flatten();
                        let tier = service_tier_from_event(&value).unwrap_or(default_service_tier);
                        visit(delta, date, tier, &current_model);
                    }
                }
            }
        }
        line.clear();
    }
    Some(())
}

fn usage_from_json(value: &Value) -> Option<Usage> {
    let input = value.get("input_tokens")?.as_u64().unwrap_or(0);
    let output = value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(Usage {
        input,
        cached: value
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_write: value
            .get("cache_write_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output,
        reasoning: value
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total: input.saturating_add(output),
    })
}

fn model_from_event(value: &Value) -> Option<&str> {
    value
        .pointer("/payload/model")
        .or_else(|| value.get("model"))
        .and_then(Value::as_str)
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

fn last_usage_from_event(value: &Value) -> Option<Usage> {
    [
        "/payload/info/last_token_usage",
        "/payload/last_token_usage",
        "/info/last_token_usage",
        "/last_token_usage",
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
    if price.long_context && usage.input > 272_000 {
        price.input *= 2.0;
        price.cached *= 2.0;
        price.cache_write *= 2.0;
        price.output *= 1.5;
    }
    let uncached = usage
        .input
        .saturating_sub(usage.cached)
        .saturating_sub(usage.cache_write);
    (uncached as f64 * price.input
        + usage.cached as f64 * price.cached
        + usage.cache_write as f64 * price.cache_write
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

fn event_local_date(value: &Value) -> Option<DateKey> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(local_date_from_utc_timestamp)
}

fn local_date_from_utc_timestamp(timestamp: &str) -> Option<DateKey> {
    let utc = utc_system_time_from_timestamp(timestamp)?;
    unsafe {
        let mut local = zeroed();
        if SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) == 0 {
            return None;
        }
        Some(date_from_system(local))
    }
}

fn utc_system_time_from_timestamp(timestamp: &str) -> Option<SYSTEMTIME> {
    let bytes = timestamp.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || !timestamp.ends_with('Z')
    {
        return None;
    }
    let year = parse_timestamp_part(bytes, 0, 4)?;
    let month = parse_timestamp_part(bytes, 5, 7)?;
    let day = parse_timestamp_part(bytes, 8, 10)?;
    let hour = parse_timestamp_part(bytes, 11, 13)?;
    let minute = parse_timestamp_part(bytes, 14, 16)?;
    let second = parse_timestamp_part(bytes, 17, 19)?;
    if !(1..=12).contains(&month)
        || day == 0
        || day > last_day_of_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(SYSTEMTIME {
        wYear: year,
        wMonth: month,
        wDayOfWeek: 0,
        wDay: day,
        wHour: hour,
        wMinute: minute,
        wSecond: second,
        wMilliseconds: 0,
    })
}

fn parse_timestamp_part(bytes: &[u8], start: usize, end: usize) -> Option<u16> {
    bytes.get(start..end)?.iter().try_fold(0u16, |value, byte| {
        byte.is_ascii_digit()
            .then_some(value * 10 + u16::from(byte - b'0'))
    })
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

fn cycle_scan_month_dirs(home: &Path, cycle_start: DateKey, today: DateKey) -> Vec<PathBuf> {
    let (year, month) = previous_month(cycle_start.year, cycle_start.month);
    cycle_month_dirs(
        home,
        DateKey {
            year,
            month,
            day: 1,
        },
        today,
    )
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

unsafe fn claim_single_instance() -> Option<InstanceGuard> {
    match try_claim_instance_mutex()? {
        Some(guard) => Some(guard),
        None => {
            for _ in 0..10 {
                let hwnd = find_app_window();
                if !hwnd.is_null() {
                    send_instance_message(hwnd, WM_SHOW_POPUP);
                    SetForegroundWindow(hwnd);
                    break;
                }
                Sleep(50);
            }
            None
        }
    }
}

unsafe fn try_claim_instance_mutex() -> Option<Option<InstanceGuard>> {
    let name = wide(INSTANCE_MUTEX_NAME);
    let handle = CreateMutexW(null(), 0, name.as_ptr());
    if handle.is_null() {
        return None;
    }
    if GetLastError() == ERROR_ALREADY_EXISTS {
        CloseHandle(handle);
        Some(None)
    } else {
        Some(Some(InstanceGuard { handle }))
    }
}

unsafe fn find_app_window() -> HWND {
    FindWindowW(
        wide(APP_WINDOW_CLASS).as_ptr(),
        wide(APP_WINDOW_TITLE).as_ptr(),
    )
}

unsafe fn send_instance_message(hwnd: HWND, message: u32) -> bool {
    let mut result = 0usize;
    SendMessageTimeoutW(hwnd, message, 0, 0, SMTO_ABORTIFHUNG, 750, &mut result) != 0
}

unsafe fn run_tray() {
    let Some(_instance_guard) = claim_single_instance() else {
        return;
    };
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    enable_native_dark_menus();
    let gdiplus_token = gdiplus_startup();
    TASKBAR_CREATED
        .set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()))
        .ok();
    SNAPSHOT
        .set(Mutex::new(Snapshot {
            config: load_config(),
            ..Snapshot::default()
        }))
        .ok();
    let instance = GetModuleHandleW(null());
    let class = wide(APP_WINDOW_CLASS);
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
        wide(APP_WINDOW_TITLE).as_ptr(),
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

    apply_window_visuals(hwnd);
    tray_icon(hwnd, NIM_ADD);
    request_refresh(hwnd);
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
    if TASKBAR_CREATED.get().is_some_and(|message| msg == *message) {
        tray_icon(hwnd, NIM_ADD);
        return 0;
    }
    match msg {
        WM_SHOW_POPUP => {
            show_popup(hwnd);
            0
        }
        WM_TRAYICON => {
            match lparam as u32 {
                WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                    KillTimer(hwnd, HIDE_TIMER_UID);
                }
                WM_LBUTTONUP => {
                    KillTimer(hwnd, HIDE_TIMER_UID);
                    toggle_popup(hwnd);
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_LBUTTONUP => {
            let x = unscale(dpi(hwnd), (lparam & 0xffff) as i16 as i32);
            let y = unscale(dpi(hwnd), ((lparam >> 16) & 0xffff) as i16 as i32);
            if x >= 340 && y < 56 {
                show_menu(hwnd);
            }
            0
        }
        WM_CONTEXTMENU => {
            show_menu(hwnd);
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            if (ID_PLAN_BASE..ID_PLAN_BASE + PLANS.len()).contains(&id) {
                set_plan(PLANS[id - ID_PLAN_BASE].id);
                apply_config_change(hwnd, false);
            } else if (ID_DAY_BASE..ID_DAY_BASE + 31).contains(&id) {
                set_cycle_day((id - ID_DAY_BASE + 1) as u16);
                apply_config_change(hwnd, true);
            } else if (ID_LANG_BASE..ID_LANG_BASE + 3).contains(&id) {
                set_language(["auto", "en", "es"][id - ID_LANG_BASE]);
                apply_config_change(hwnd, false);
            } else {
                match id {
                    ID_REFRESH => {
                        request_refresh(hwnd);
                        show_popup(hwnd);
                    }
                    ID_ALL_TIME => {
                        request_all_time(hwnd);
                        show_popup(hwnd);
                    }
                    ID_CUSTOM_MINUS_10 => adjust_custom_amount(-10.0),
                    ID_CUSTOM_MINUS_1 => adjust_custom_amount(-1.0),
                    ID_CUSTOM_PLUS_1 => adjust_custom_amount(1.0),
                    ID_CUSTOM_PLUS_10 => adjust_custom_amount(10.0),
                    ID_OPEN_CONFIG => open_config(hwnd),
                    ID_OPEN_USAGE => open_url(hwnd, USAGE_URL),
                    ID_EXIT => {
                        DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                if (ID_CUSTOM_MINUS_10..=ID_CUSTOM_PLUS_10).contains(&id) {
                    apply_config_change(hwnd, false);
                }
            }
            0
        }
        WM_TIMER => {
            if wparam == HIDE_TIMER_UID {
                KillTimer(hwnd, HIDE_TIMER_UID);
                hide_popup(hwnd);
            } else {
                request_refresh(hwnd);
            }
            0
        }
        WM_REFRESHED | WM_ALL_TIME_REFRESHED => {
            tray_icon(hwnd, NIM_MODIFY);
            InvalidateRect(hwnd, null(), 0);
            if msg == WM_REFRESHED && REFRESH_PENDING.swap(false, Ordering::AcqRel) {
                request_refresh(hwnd);
            }
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
            apply_window_visuals(hwnd);
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            enable_native_dark_menus();
            apply_window_visuals(hwnd);
            tray_icon(hwnd, NIM_MODIFY);
            InvalidateRect(hwnd, null(), 1);
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xffff) == WA_INACTIVE as usize {
                SetTimer(hwnd, HIDE_TIMER_UID, 250, None);
            } else {
                KillTimer(hwnd, HIDE_TIMER_UID);
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

fn request_refresh(hwnd: HWND) {
    if REFRESHING.swap(true, Ordering::AcqRel) {
        REFRESH_PENDING.store(true, Ordering::Release);
        return;
    }
    let hwnd = hwnd as usize;
    thread::spawn(move || {
        let scan_config = load_config();
        let mut snap = scan_month(&scan_config).unwrap_or_else(|error| Snapshot {
            error: Some(error),
            config: scan_config.clone(),
            ..Snapshot::default()
        });
        let latest_config = load_config();
        if latest_config.cycle_day != scan_config.cycle_day {
            REFRESH_PENDING.store(true, Ordering::Release);
        }
        if let Some(lock) = SNAPSHOT.get() {
            if let Ok(mut current) = lock.lock() {
                snap.all_time = current.all_time.clone();
                snap.all_time_updated = current.all_time_updated.clone();
                snap.config = latest_config;
                *current = snap;
            }
        }
        REFRESHING.store(false, Ordering::Release);
        unsafe {
            PostMessageW(hwnd as HWND, WM_REFRESHED, 0, 0);
        }
    });
}

fn request_all_time(hwnd: HWND) {
    if ALL_TIME_REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(mut snap) = SNAPSHOT
        .get()
        .and_then(|lock| lock.lock().ok().map(|snap| snap.clone()))
    else {
        ALL_TIME_REFRESHING.store(false, Ordering::Release);
        return;
    };
    let hwnd = hwnd as usize;
    thread::spawn(move || {
        calculate_all_time(&mut snap);
        if let Some(lock) = SNAPSHOT.get() {
            if let Ok(mut current) = lock.lock() {
                current.all_time = snap.all_time;
                current.all_time_updated = snap.all_time_updated;
                for model in snap.unknown_models {
                    if !current.unknown_models.contains(&model) {
                        current.unknown_models.push(model);
                    }
                }
                current.unknown_models.sort();
            }
        }
        ALL_TIME_REFRESHING.store(false, Ordering::Release);
        unsafe {
            PostMessageW(hwnd as HWND, WM_ALL_TIME_REFRESHED, 0, 0);
        }
    });
}

unsafe fn apply_config_change(hwnd: HWND, rescan: bool) {
    let config = load_config();
    if let Some(lock) = SNAPSHOT.get() {
        if let Ok(mut snap) = lock.lock() {
            snap.config = config;
        }
    }
    tray_icon(hwnd, NIM_MODIFY);
    InvalidateRect(hwnd, null(), 0);
    if rescan {
        request_refresh(hwnd);
    }
    show_popup(hwnd);
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
    data.hIcon = cached_themed_tray_icon(windows_system_dark()).unwrap_or_else(|| app_icon());

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
}

unsafe fn focus_tray_icon(hwnd: HWND) {
    let mut data: NOTIFYICONDATAW = zeroed();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_UID;
    Shell_NotifyIconW(NIM_SETFOCUS, &data);
}

unsafe fn toggle_popup(hwnd: HWND) {
    if IsWindowVisible(hwnd) != 0 {
        hide_popup(hwnd);
        return;
    }
    show_popup(hwnd);
}

unsafe fn show_popup(hwnd: HWND) {
    let icon_rect = tray_icon_rect(hwnd);
    let mut pt = icon_rect
        .map(|rect| POINT {
            x: (rect.left + rect.right) / 2,
            y: rect.top,
        })
        .unwrap_or(POINT { x: 0, y: 0 });
    if icon_rect.is_none() {
        GetCursorPos(&mut pt);
    }
    let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let target_dpi = monitor_dpi(monitor);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..zeroed()
    };
    GetMonitorInfoW(monitor, &mut info);
    let w = z(target_dpi, POPUP_W);
    let h = z(target_dpi, POPUP_H);
    let gap = z(target_dpi, POPUP_GAP);
    let (x, y) = if let Some(mut exclude) = icon_rect {
        exclude.left -= gap;
        exclude.top -= gap;
        exclude.right += gap;
        exclude.bottom += gap;
        let anchor = POINT {
            x: exclude.right,
            y: exclude.top,
        };
        let size = SIZE { cx: w, cy: h };
        let mut popup = zeroed();
        if CalculatePopupWindowPosition(
            &anchor,
            &size,
            TPM_RIGHTALIGN | TPM_BOTTOMALIGN | TPM_VERTICAL | TPM_WORKAREA,
            &exclude,
            &mut popup,
        ) != 0
        {
            (popup.left, popup.top)
        } else {
            (
                pt.x - w,
                anchored_popup_y(pt.y, h, info.rcWork.top, info.rcWork.bottom, gap),
            )
        }
    } else {
        (
            (pt.x - w + z(target_dpi, 20)).clamp(info.rcWork.left, info.rcWork.right - w),
            anchored_popup_y(pt.y, h, info.rcWork.top, info.rcWork.bottom, gap),
        )
    };
    let (x, y) = clamp_popup_origin(x, y, w, h, info.rcWork, gap);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    apply_window_visuals(hwnd);
    InvalidateRect(hwnd, null(), 1);
    let (show_animation, _) = popup_animation_flags(pt, info.rcWork);
    if AnimateWindow(
        hwnd,
        POPUP_ANIMATION_MS,
        AW_ACTIVATE | AW_SLIDE | show_animation,
    ) == 0
    {
        ShowWindow(hwnd, SW_SHOW);
    }
    SetForegroundWindow(hwnd);
}

unsafe fn hide_popup(hwnd: HWND) {
    if IsWindowVisible(hwnd) == 0 {
        return;
    }
    let mut rect = zeroed();
    GetWindowRect(hwnd, &mut rect);
    let monitor = MonitorFromPoint(
        POINT {
            x: (rect.left + rect.right) / 2,
            y: (rect.top + rect.bottom) / 2,
        },
        MONITOR_DEFAULTTONEAREST,
    );
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..zeroed()
    };
    GetMonitorInfoW(monitor, &mut info);
    let (_, hide_animation) = popup_animation_flags(
        POINT {
            x: (rect.left + rect.right) / 2,
            y: (rect.top + rect.bottom) / 2,
        },
        info.rcWork,
    );
    if AnimateWindow(
        hwnd,
        POPUP_ANIMATION_MS,
        AW_HIDE | AW_SLIDE | hide_animation,
    ) == 0
    {
        ShowWindow(hwnd, SW_HIDE);
    }
}

unsafe fn tray_icon_rect(hwnd: HWND) -> Option<RECT> {
    let identifier = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        ..zeroed()
    };
    let mut rect = zeroed();
    (Shell_NotifyIconGetRect(&identifier, &mut rect) >= 0).then_some(rect)
}

unsafe fn monitor_dpi(monitor: HMONITOR) -> i32 {
    let (mut x, mut y) = (96, 96);
    if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) >= 0 {
        x as i32
    } else {
        96
    }
}

unsafe fn apply_window_visuals(hwnd: HWND) {
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

    let corner = DWMWCP_ROUND;
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
}

unsafe fn show_menu(hwnd: HWND) {
    enable_native_dark_menus();
    let config = SNAPSHOT
        .get()
        .and_then(|s| s.lock().ok().map(|s| s.config.clone()))
        .unwrap_or_else(load_config);
    let lang = current_lang(&config);
    let menu = CreatePopupMenu();
    let plans = CreatePopupMenu();
    for (index, plan) in PLANS.iter().enumerate() {
        AppendMenuW(
            plans,
            MF_STRING,
            ID_PLAN_BASE + index,
            wide(&plan_menu_label(plan, lang)).as_ptr(),
        );
    }
    CheckMenuRadioItem(
        plans,
        ID_PLAN_BASE as u32,
        (ID_PLAN_BASE + PLANS.len() - 1) as u32,
        (ID_PLAN_BASE
            + PLANS
                .iter()
                .position(|plan| plan.id == config.plan)
                .unwrap_or(2)) as u32,
        MF_BYCOMMAND,
    );
    AppendMenuW(
        menu,
        MF_POPUP,
        plans as usize,
        wide(&format!(
            "{} · {}",
            t(lang, "Plan", "Plan"),
            plan_name(current_plan(&config), lang)
        ))
        .as_ptr(),
    );

    let days = CreatePopupMenu();
    for day in 1..=31 {
        let column = if day == 11 || day == 21 {
            MF_MENUBARBREAK
        } else {
            0
        };
        AppendMenuW(
            days,
            MF_STRING | column,
            ID_DAY_BASE + day - 1,
            wide(&day.to_string()).as_ptr(),
        );
    }
    CheckMenuRadioItem(
        days,
        ID_DAY_BASE as u32,
        (ID_DAY_BASE + 30) as u32,
        (ID_DAY_BASE + config.cycle_day.clamp(1, 31) as usize - 1) as u32,
        MF_BYCOMMAND,
    );
    AppendMenuW(
        menu,
        MF_POPUP,
        days as usize,
        wide(&format!(
            "{} · {}",
            t(lang, "Cycle starts", "Inicio del ciclo"),
            config.cycle_day
        ))
        .as_ptr(),
    );

    let languages = CreatePopupMenu();
    for (index, label) in ["Automatico / Automatic", "English", "Español"]
        .iter()
        .enumerate()
    {
        AppendMenuW(
            languages,
            MF_STRING,
            ID_LANG_BASE + index,
            wide(label).as_ptr(),
        );
    }
    let language_index = match config.language.as_str() {
        "en" => 1,
        "es" => 2,
        _ => 0,
    };
    CheckMenuRadioItem(
        languages,
        ID_LANG_BASE as u32,
        (ID_LANG_BASE + 2) as u32,
        (ID_LANG_BASE + language_index) as u32,
        MF_BYCOMMAND,
    );
    AppendMenuW(
        menu,
        MF_POPUP,
        languages as usize,
        wide(t(lang, "Language", "Idioma")).as_ptr(),
    );

    let custom = CreatePopupMenu();
    AppendMenuW(
        custom,
        MF_STRING | MF_DISABLED,
        0,
        wide(&format!(
            "{} / {}",
            money(plan_usd(&config)),
            t(lang, "month", "mes")
        ))
        .as_ptr(),
    );
    AppendMenuW(
        custom,
        MF_STRING,
        ID_CUSTOM_MINUS_10,
        wide("- $10").as_ptr(),
    );
    AppendMenuW(custom, MF_STRING, ID_CUSTOM_MINUS_1, wide("- $1").as_ptr());
    AppendMenuW(custom, MF_STRING, ID_CUSTOM_PLUS_1, wide("+ $1").as_ptr());
    AppendMenuW(custom, MF_STRING, ID_CUSTOM_PLUS_10, wide("+ $10").as_ptr());
    AppendMenuW(
        menu,
        MF_POPUP,
        custom as usize,
        wide(t(
            lang,
            "Custom monthly amount",
            "Monto mensual personalizado",
        ))
        .as_ptr(),
    );

    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(
        menu,
        MF_STRING,
        ID_REFRESH,
        wide(t(lang, "Reload", "Recargar")).as_ptr(),
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
        wide(t(
            lang,
            "Advanced config file",
            "Archivo de config avanzado",
        ))
        .as_ptr(),
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
    TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_WORKAREA,
        pt.x,
        pt.y,
        0,
        hwnd,
        null(),
    );
    PostMessageW(hwnd, WM_NULL, 0, 0);
    focus_tray_icon(hwnd);
    DestroyMenu(menu);
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
    SetBkMode(hdc, TRANSPARENT as i32);
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));

    draw_font_fit(
        ui,
        &format!("Codex · {}", plan_name(plan, lang)),
        [CONTENT_PAD, CONTENT_PAD, 216, 30],
        theme.text,
        20,
        15,
        700,
    );
    draw_right(
        ui,
        &cycle_days_header_text(lang, snap.cycle_days_left),
        [224, CONTENT_PAD + 7, 118, 18],
        theme.subtle,
    );
    draw_font_center(
        ui,
        "⋯",
        [348, CONTENT_PAD - 2, 24, 24],
        theme.muted,
        18,
        600,
    );

    let pct = if plan_usd > 0.0 {
        snap.month.cost / plan_usd
    } else {
        0.0
    };
    let pct_label = if plan_usd > 0.0 {
        format!("{:.0}%", pct * 100.0)
    } else {
        "--".to_string()
    };

    ring(ui, [34, 78, 104, 104], pct, theme);
    draw_font_center_fit(ui, &pct_label, [45, 108, 82, 30], theme.text, 27, 15, 650);
    draw_center(
        ui,
        t(lang, "recovered", "recuperado"),
        [47, 138, 78, 20],
        theme.muted,
    );

    let month_cost = money(snap.month.cost);
    draw_font_fit(ui, &month_cost, [166, 78, 202, 34], theme.text, 25, 15, 700);
    draw(
        ui,
        t(lang, "API-equivalent value", "valor API equivalente"),
        [166, 110, 202, 18],
        theme.muted,
    );

    if plan_usd > 0.0 && snap.month.cost >= plan_usd {
        draw_font_fit(
            ui,
            &format!(
                "{} {}",
                money(snap.month.cost - plan_usd),
                t(lang, "saved this cycle", "ahorrados este ciclo")
            ),
            [166, 136, 202, 20],
            theme.success,
            13,
            10,
            600,
        );
    } else if plan_usd > 0.0 {
        draw_font_fit(
            ui,
            &format!(
                "{} {}",
                money(plan_usd - snap.month.cost),
                t(lang, "to break even", "para amortizar")
            ),
            [166, 136, 202, 20],
            solid_from_argb(theme.accent_argb),
            13,
            10,
            600,
        );
    } else {
        draw_font_fit(
            ui,
            plan_limit(plan, lang),
            [166, 136, 202, 20],
            theme.subtle,
            12,
            9,
            400,
        );
    }
    draw_font_fit(
        ui,
        &format!("{} · {}", t(lang, "Today", "Hoy"), money(snap.today.cost)),
        [166, 160, 202, 20],
        theme.muted,
        12,
        10,
        400,
    );

    let footer_y = POPUP_H - CONTENT_PAD - 18;
    if let Some(error) = &snap.error {
        draw(ui, error, [CONTENT_PAD, footer_y, 210, 18], theme.danger);
    } else if !snap.unknown_models.is_empty() {
        draw(
            ui,
            &format!(
                "{} {}",
                snap.unknown_models.len(),
                t(lang, "models without price", "modelos sin precio")
            ),
            [CONTENT_PAD, footer_y, 204, 18],
            theme.warning,
        );
    }
    let status = if ALL_TIME_REFRESHING.load(Ordering::Relaxed) {
        t(lang, "calculating total…", "calculando total…").to_string()
    } else if REFRESHING.load(Ordering::Relaxed) {
        t(lang, "updating…", "actualizando…").to_string()
    } else {
        format!("{} {}", t(lang, "updated", "actualizado"), snap.updated)
    };
    draw_right(ui, &status, [218, footer_y, 150, 18], theme.subtle);

    EndPaint(hwnd, &ps);
}

unsafe fn fill(hdc: HDC, rect: RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, &rect, brush);
    DeleteObject(brush);
}

unsafe fn ring(ui: Ui, rect: [i32; 4], pct: f64, theme: Theme) {
    draw_arc(ui, rect, 12, 0.0, 360.0, colorref_to_argb(255, theme.track));
    let sweep = 360.0 * pct.clamp(0.0, 1.0) as f32;
    if sweep > 0.0 {
        let color = if pct >= 1.0 {
            colorref_to_argb(255, theme.success)
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

unsafe fn draw_font_fit(
    ui: Ui,
    text: &str,
    rect: [i32; 4],
    color: COLORREF,
    size: i32,
    min_size: i32,
    weight: i32,
) {
    let size = fitted_font_size(ui, text, rect[2], size, min_size, weight);
    draw_font_aligned(ui, text, rect, color, size, weight, DT_LEFT);
}

unsafe fn draw_font_center_fit(
    ui: Ui,
    text: &str,
    rect: [i32; 4],
    color: COLORREF,
    size: i32,
    min_size: i32,
    weight: i32,
) {
    let size = fitted_font_size(ui, text, rect[2], size, min_size, weight);
    draw_font_aligned(ui, text, rect, color, size, weight, DT_CENTER);
}

unsafe fn fitted_font_size(
    ui: Ui,
    text: &str,
    max_width: i32,
    size: i32,
    min_size: i32,
    weight: i32,
) -> i32 {
    let min_size = min_size.min(size);
    let mut current = size;
    while current > min_size && font_text_width(ui, text, current, weight) > max_width {
        current -= 1;
    }
    current
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

fn windows_system_dark() -> bool {
    personalize_dword("SystemUsesLightTheme", 1) == 0
}

unsafe fn enable_native_dark_menus() {
    let library = LoadLibraryExW(
        wide("uxtheme.dll").as_ptr(),
        null_mut(),
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    );
    if library.is_null() {
        return;
    }

    // ponytail: uxtheme ordinals are the smallest native-menu fix; replace
    // with custom menus if Windows stops exporting them.
    if let Some(proc) = GetProcAddress(library, std::ptr::without_provenance::<u8>(135)) {
        let set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32 =
            std::mem::transmute(proc);
        set_preferred_app_mode(if windows_apps_dark() || windows_system_dark() {
            2
        } else {
            3
        });
    }
    if let Some(proc) = GetProcAddress(library, std::ptr::without_provenance::<u8>(136)) {
        let flush_menu_themes: unsafe extern "system" fn() = std::mem::transmute(proc);
        flush_menu_themes();
    }
    FreeLibrary(library);
}

unsafe fn cached_themed_tray_icon(dark: bool) -> Option<HICON> {
    let icons = THEMED_TRAY_ICONS.get_or_init(|| {
        [
            themed_tray_icon(false).unwrap_or(null_mut()) as usize,
            themed_tray_icon(true).unwrap_or(null_mut()) as usize,
        ]
    });
    let icon = icons[usize::from(dark)] as HICON;
    (!icon.is_null()).then_some(icon)
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
    let current_amount = plan_usd(&config);
    config.plan = id.to_string();
    config.monthly_usd_override = (id == "custom").then_some(
        config
            .monthly_usd_override
            .unwrap_or(current_amount.max(20.0)),
    );
    write_config(&config);
}

fn set_cycle_day(day: u16) {
    let mut config = load_config();
    config.cycle_day = day.clamp(1, 31);
    write_config(&config);
}

fn set_language(language: &str) {
    if !matches!(language, "auto" | "en" | "es") {
        return;
    }
    let mut config = load_config();
    config.language = language.to_string();
    write_config(&config);
}

fn adjust_custom_amount(delta: f64) {
    let mut config = load_config();
    let amount = config
        .monthly_usd_override
        .unwrap_or_else(|| plan_usd(&config));
    config.plan = "custom".to_string();
    config.monthly_usd_override = Some((amount + delta).clamp(0.0, 1_000_000.0));
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

fn anchored_popup_y(cursor_y: i32, popup_h: i32, work_top: i32, work_bottom: i32, gap: i32) -> i32 {
    let max_y = (work_bottom - popup_h - gap).max(work_top);
    (cursor_y - popup_h - gap).clamp(work_top, max_y)
}

fn clamp_popup_origin(x: i32, y: i32, w: i32, h: i32, work: RECT, gap: i32) -> (i32, i32) {
    (
        x.clamp(work.left, (work.right - w).max(work.left)),
        y.clamp(work.top, (work.bottom - h - gap).max(work.top)),
    )
}

fn popup_animation_flags(anchor: POINT, work: RECT) -> (u32, u32) {
    let distances = [
        (anchor.y - work.bottom).abs(),
        (anchor.y - work.top).abs(),
        (anchor.x - work.right).abs(),
        (anchor.x - work.left).abs(),
    ];
    match distances
        .iter()
        .enumerate()
        .min_by_key(|(_, distance)| *distance)
        .map(|(edge, _)| edge)
        .unwrap_or(0)
    {
        0 => (AW_VER_NEGATIVE, AW_VER_POSITIVE),
        1 => (AW_VER_POSITIVE, AW_VER_NEGATIVE),
        2 => (AW_HOR_NEGATIVE, AW_HOR_POSITIVE),
        _ => (AW_HOR_POSITIVE, AW_HOR_NEGATIVE),
    }
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

    fn usage_line_at(
        timestamp: &str,
        input: u64,
        cached: u64,
        output: u64,
        reasoning: u64,
        total: u64,
    ) -> String {
        serde_json::json!({
            "timestamp": timestamp,
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

    fn usage_line_with_last_at(timestamp: &str, total_usage: Usage, last_usage: Usage) -> String {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": total_usage.input,
                        "cached_input_tokens": total_usage.cached,
                        "output_tokens": total_usage.output,
                        "reasoning_output_tokens": total_usage.reasoning,
                        "total_tokens": total_usage.total,
                    },
                    "last_token_usage": {
                        "input_tokens": last_usage.input,
                        "cached_input_tokens": last_usage.cached,
                        "output_tokens": last_usage.output,
                        "reasoning_output_tokens": last_usage.reasoning,
                        "total_tokens": last_usage.total,
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

    fn turn_context_line(model: &str) -> String {
        serde_json::json!({
            "type": "turn_context",
            "payload": { "model": model }
        })
        .to_string()
    }

    #[test]
    fn price_uses_longest_matching_model_prefix() {
        let price = price_for_model("gpt-5.4-mini-2026-04-28").unwrap();
        assert_eq!(price.input, 0.75);
        assert_eq!(price.output, 4.5);

        let terra = price_for_model("gpt-5.6-terra-2026-07-01").unwrap();
        assert_eq!(terra.input, 2.5);
        assert_eq!(terra.cached, 0.25);
        assert_eq!(terra.output, 15.0);
    }

    #[test]
    fn gpt_5_6_prices_cache_writes_and_long_context() {
        let mut unknown = HashSet::new();
        let short = Usage {
            input: 100_000,
            cached: 0,
            cache_write: 0,
            output: 0,
            reasoning: 0,
            total: 100_000,
        };
        assert_eq!(
            cost(short, "gpt-5.6-luna", ServiceTier::Standard, &mut unknown),
            0.1
        );

        let cache_write = Usage {
            input: 100_000,
            cache_write: 100_000,
            ..Usage::default()
        };
        assert_eq!(
            cost(
                cache_write,
                "gpt-5.6-sol",
                ServiceTier::Standard,
                &mut unknown
            ),
            0.625
        );

        let long = Usage {
            input: 272_001,
            output: 100_000,
            total: 372_001,
            ..Usage::default()
        };
        assert_eq!(
            cost(long, "gpt-5.6-terra", ServiceTier::Standard, &mut unknown),
            3.610005
        );
        assert!(unknown.is_empty());
    }

    #[test]
    fn fast_tier_multiplies_supported_models_only() {
        let usage = Usage {
            input: 1_000_000,
            cached: 0,
            cache_write: 0,
            output: 0,
            reasoning: 0,
            total: 1_000_000,
        };
        let mut unknown = HashSet::new();

        assert_eq!(
            cost(usage, "gpt-5.5", ServiceTier::Standard, &mut unknown),
            10.0
        );
        assert_eq!(
            cost(usage, "gpt-5.5", ServiceTier::Fast, &mut unknown),
            25.0
        );
        assert_eq!(
            cost(usage, "gpt-5.4", ServiceTier::Fast, &mut unknown),
            10.0
        );
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
            cache_write: 0,
            output: 5,
            reasoning: 1,
            total: 17,
        };
        let previous = Usage {
            input: 10,
            cached: 3,
            cache_write: 0,
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
    fn parse_session_prices_each_turn_with_its_recorded_model() {
        let contents = [
            turn_context_line("gpt-5.6-sol"),
            usage_line(100_000, 0, 0, 0, 100_000),
            turn_context_line("gpt-5.6-luna"),
            usage_line(200_000, 0, 0, 0, 200_000),
        ]
        .join("\n");
        let path = temp_jsonl(&contents);
        let mut unknown = HashSet::new();
        let period = parse_session(&path, "gpt-5.5", ServiceTier::Standard, &mut unknown).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.cost, 0.6);
        assert!(unknown.is_empty());
    }

    #[test]
    fn parse_session_ignores_non_billable_total_only_events() {
        let path = temp_jsonl(
            &serde_json::json!({
                "type": "token_count",
                "total_token_usage": {
                    "input_tokens": 0,
                    "cached_input_tokens": 0,
                    "output_tokens": 0,
                    "reasoning_output_tokens": 0,
                    "total_tokens": 116_457,
                }
            })
            .to_string(),
        );
        let mut unknown = HashSet::new();
        let period = parse_session(&path, "gpt-5.6-sol", ServiceTier::Standard, &mut unknown);
        fs::remove_file(path).unwrap();

        assert!(period.is_none());
        assert!(unknown.is_empty());
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

        assert_eq!(period.cost, 25.0);
        assert!(unknown.is_empty());
    }

    #[test]
    fn parse_session_uses_standard_event_before_fast_default() {
        let line = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "service_tier": "standard",
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
        let period = parse_session(&path, "gpt-5.5", ServiceTier::Fast, &mut unknown).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.cost, 10.0);
        assert!(unknown.is_empty());
    }

    #[test]
    fn parse_session_between_counts_only_events_inside_cycle() {
        let contents = [
            usage_line_at("2026-05-23T12:00:00.000Z", 1_000_000, 0, 0, 0, 1_000_000),
            usage_line_at("2026-05-24T12:00:00.000Z", 1_500_000, 0, 0, 0, 1_500_000),
        ]
        .join("\n");
        let path = temp_jsonl(&contents);
        let mut unknown = HashSet::new();
        let period = parse_session_between(
            &path,
            "gpt-5.5",
            ServiceTier::Standard,
            DateKey {
                year: 2026,
                month: 5,
                day: 24,
            },
            DateKey {
                year: 2026,
                month: 5,
                day: 24,
            },
            &mut unknown,
        )
        .unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.calls, 1);
        assert_eq!(period.usage.input, 500_000);
        assert_eq!(period.cost, 5.0);
        assert!(unknown.is_empty());
    }

    #[test]
    fn parse_session_current_builds_cycle_and_today_in_one_pass() {
        let contents = [
            usage_line_at("2026-07-08T12:00:00.000Z", 100, 0, 0, 0, 100),
            usage_line_at("2026-07-09T12:00:00.000Z", 200, 0, 0, 0, 200),
        ]
        .join("\n");
        let path = temp_jsonl(&contents);
        let mut unknown = HashSet::new();
        let (cycle, today) = parse_session_current(
            &path,
            "gpt-5.6-sol",
            ServiceTier::Standard,
            DateKey {
                year: 2026,
                month: 7,
                day: 6,
            },
            DateKey {
                year: 2026,
                month: 7,
                day: 9,
            },
            &mut unknown,
        );
        fs::remove_file(path).unwrap();

        assert_eq!(cycle.unwrap().usage.input, 200);
        assert_eq!(today.unwrap().usage.input, 100);
        assert!(unknown.is_empty());
    }

    #[test]
    fn parse_session_between_uses_last_usage_for_first_observed_in_range() {
        let contents = usage_line_with_last_at(
            "2026-05-24T12:00:00.000Z",
            Usage {
                input: 1_500_000,
                cached: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
                total: 1_500_000,
            },
            Usage {
                input: 500_000,
                cached: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
                total: 500_000,
            },
        );
        let path = temp_jsonl(&contents);
        let mut unknown = HashSet::new();
        let period = parse_session_between(
            &path,
            "gpt-5.5",
            ServiceTier::Standard,
            DateKey {
                year: 2026,
                month: 5,
                day: 24,
            },
            DateKey {
                year: 2026,
                month: 5,
                day: 24,
            },
            &mut unknown,
        )
        .unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.calls, 1);
        assert_eq!(period.usage.input, 500_000);
        assert_eq!(period.cost, 5.0);
        assert!(unknown.is_empty());
    }

    #[test]
    fn service_tier_from_event_reads_speed_alias() {
        let value = serde_json::json!({
            "payload": {
                "info": {
                    "speed": "fast"
                }
            }
        });

        assert_eq!(service_tier_from_event(&value), Some(ServiceTier::Fast));
    }

    #[test]
    fn service_tier_from_toml_reads_fast_setting() {
        let contents = r#"
            model = "gpt-5.5"
            service_tier = "fast"
            [features]
            fast_mode = true
        "#;
        assert_eq!(service_tier_from_toml(contents), Some(ServiceTier::Fast));
    }

    #[test]
    fn service_tier_from_toml_requires_fast_feature_flag() {
        assert_eq!(
            service_tier_from_toml("service_tier = \"fast\""),
            Some(ServiceTier::Standard)
        );
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
    fn current_cycle_start_uses_today_on_cycle_day() {
        let start = current_cycle_start(
            DateKey {
                year: 2026,
                month: 5,
                day: 24,
            },
            24,
        );
        assert_eq!(
            start,
            DateKey {
                year: 2026,
                month: 5,
                day: 24,
            }
        );
    }

    #[test]
    fn cycle_scan_month_dirs_includes_previous_month() {
        let dirs = cycle_scan_month_dirs(
            Path::new("C:\\codex"),
            DateKey {
                year: 2026,
                month: 5,
                day: 24,
            },
            DateKey {
                year: 2026,
                month: 5,
                day: 26,
            },
        );

        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].ends_with(Path::new("sessions\\2026\\04")));
        assert!(dirs[1].ends_with(Path::new("sessions\\2026\\05")));
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
        assert!(plus.limits_en.contains("Sol 15-90"));
        assert!(pro.limits_en.contains("Sol 300-1800"));
        assert!(CREDIT_RATES.contains("Terra 62.5/6.25/375"));
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
    fn popup_origin_stays_above_taskbar() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        };
        assert_eq!(
            clamp_popup_origin(1600, 900, 392, 226, work, 12),
            (1528, 794)
        );
    }

    #[test]
    fn popup_animation_enters_from_nearest_taskbar_edge() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        };
        assert_eq!(
            popup_animation_flags(POINT { x: 1800, y: 1060 }, work),
            (AW_VER_NEGATIVE, AW_VER_POSITIVE)
        );
    }

    #[test]
    fn all_time_is_on_demand_by_default() {
        assert!(Snapshot::default().all_time.is_none());
    }
}
