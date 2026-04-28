#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    env,
    fs::{self, File},
    io::{BufRead, BufReader},
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{Mutex, OnceLock},
};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM},
    Globalization::GetUserDefaultUILanguage,
    Graphics::Gdi::{
        BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
        EndPaint, FillRect, FillRgn, FrameRgn, GetMonitorInfoW, GetStockObject, InvalidateRect,
        MonitorFromPoint, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, DEFAULT_GUI_FONT,
        DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, HDC, HRGN, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, TRANSPARENT,
    },
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetLocalTime},
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
const ID_PLAN_BASE: usize = 1100;
const POPUP_W: i32 = 336;
const POPUP_H: i32 = 244;
const POPUP_GAP: i32 = 28;
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
    "Credits/1M tokens: 5.5 125/12.5/750; 5.4 62.5/6.25/375; mini 18.75/1.875/113";

#[derive(Clone)]
struct Config {
    plan: String,
    monthly_usd_override: Option<f64>,
    language: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            plan: "plus".to_string(),
            monthly_usd_override: None,
            language: "auto".to_string(),
        }
    }
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
            unknown_models: vec![],
            assumed_models: 0,
            error: None,
        }
    }
}

static SNAPSHOT: OnceLock<Mutex<Snapshot>> = OnceLock::new();

#[derive(Clone, Copy)]
struct Ui {
    hdc: HDC,
    dpi: i32,
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
    let mut snap = scan_month().unwrap_or_else(|error| Snapshot {
        error: Some(error),
        ..Snapshot::default()
    });
    snap.config = load_config();
    if include_all_time {
        calculate_all_time(&mut snap);
    }
    let plan = current_plan(&snap.config);
    let plan_usd = plan_usd(&snap.config);
    println!("Codex savings tray");
    println!("Home: {}", snap.codex_home.display());
    println!("Plan: {} ({}/month)", plan.en, money(plan_usd));
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

fn scan_month() -> Result<Snapshot, String> {
    let home = codex_home();
    let now = local_time();
    let metadata = load_metadata(&home);
    let month_dir = home
        .join("sessions")
        .join(format!("{:04}", now.wYear))
        .join(format!("{:02}", now.wMonth));
    let mut snap = Snapshot {
        updated: format!("{:02}:{:02}", now.wHour, now.wMinute),
        codex_home: home,
        ..Snapshot::default()
    };

    if !month_dir.exists() {
        return Ok(snap);
    }

    let mut unknown = HashSet::new();
    for path in jsonl_files(&month_dir) {
        let key = path_key(&path);
        let model = metadata
            .get(&key)
            .cloned()
            .or_else(|| env::var("CODEX_SAVINGS_MODEL").ok())
            .unwrap_or_else(|| {
                snap.assumed_models += 1;
                "gpt-5.5".to_string()
            });

        if let Some(session) = parse_session(&path, &model, &mut unknown) {
            snap.month.add(session.clone());
            if is_today_file(&path, now) {
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
        if let Some(session) = parse_session(&path, &model, &mut unknown) {
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

fn parse_session(path: &Path, model: &str, unknown: &mut HashSet<String>) -> Option<Period> {
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
        let Some(usage) = value
            .pointer("/payload/info/total_token_usage")
            .and_then(usage_from_json)
        else {
            continue;
        };
        let delta = match usage.total.cmp(&previous.total) {
            Ordering::Less => usage,
            _ => usage.delta(previous),
        };
        previous = usage;
        if !delta.any() {
            continue;
        }
        period.usage.add(delta);
        period.cost += cost(delta, model, unknown);
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

fn cost(usage: Usage, model: &str, unknown: &mut HashSet<String>) -> f64 {
    let Some(price) = price_for_model(model) else {
        unknown.insert(model.to_string());
        return 0.0;
    };
    let uncached = usage.input.saturating_sub(usage.cached);
    (uncached as f64 * price.input
        + usage.cached as f64 * price.cached
        + usage.output as f64 * price.output)
        / 1_000_000.0
}

fn price_for_model(model: &str) -> Option<Price> {
    PRICES
        .iter()
        .filter(|(prefix, _)| model.starts_with(*prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, price)| *price)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

fn is_today_file(path: &Path, now: SYSTEMTIME) -> bool {
    let day = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str());
    let month = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str());
    let year = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str());
    day == Some(&format!("{:02}", now.wDay))
        && month == Some(&format!("{:02}", now.wMonth))
        && year == Some(&format!("{:04}", now.wYear))
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
        return;
    }

    refresh(hwnd);
    tray_icon(hwnd, NIM_ADD);
    SetTimer(hwnd, TIMER_UID, 300_000, None);

    let mut msg = zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
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
            shape_window(hwnd, rect.right - rect.left, rect.bottom - rect.top);
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
    let mut snap = scan_month().unwrap_or_else(|error| Snapshot {
        error: Some(error),
        ..Snapshot::default()
    });
    snap.config = load_config();
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
    let mut data: NOTIFYICONDATAW = zeroed();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_UID;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAYICON;
    data.hIcon = app_icon();

    let tip = SNAPSHOT
        .get()
        .and_then(|s| {
            s.lock().ok().map(|s| {
                let lang = current_lang(&s.config);
                let plan = current_plan(&s.config);
                let plan_usd = plan_usd(&s.config);
                let mut tip = format!(
                    "Codex {}: {} {}",
                    plan_name(plan, lang),
                    money(s.month.cost),
                    t(lang, "this month", "este mes")
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
        })
        .unwrap_or_else(|| "Codex savings".to_string());
    copy_wide(&tip, &mut data.szTip);
    Shell_NotifyIconW(action, &data);
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
    let y = (pt.y - h - scale(hwnd, POPUP_GAP)).clamp(info.rcWork.top, info.rcWork.bottom - h);
    shape_window(hwnd, w, h);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    SetForegroundWindow(hwnd);
}

unsafe fn shape_window(hwnd: HWND, w: i32, h: i32) {
    let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, scale(hwnd, 24), scale(hwnd, 24));
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

unsafe fn paint(hwnd: HWND) {
    let snap = SNAPSHOT
        .get()
        .and_then(|s| s.lock().ok().map(|s| s.clone()))
        .unwrap_or_default();
    let dpi = dpi(hwnd);
    let lang = current_lang(&snap.config);
    let plan = current_plan(&snap.config);
    let plan_usd = plan_usd(&snap.config);
    let mut ps = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let ui = Ui { hdc, dpi };
    let mut rect = zeroed();
    GetClientRect(hwnd, &mut rect);

    fill(hdc, rect, rgb(248, 249, 247));
    round_frame(ui, [0, 0, POPUP_W, POPUP_H], 12, rgb(196, 201, 196));
    SetBkMode(hdc, TRANSPARENT as i32);
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));

    draw(ui, "Codex", [18, 16, 300, 20], rgb(61, 67, 73));
    let headline = money(snap.month.cost);
    draw_big(ui, &headline, [18, 46, 180, 42], rgb(23, 28, 32));
    draw(
        ui,
        t(
            lang,
            "API value used this month",
            "valor API usado este mes",
        ),
        [18, 88, 220, 20],
        rgb(95, 104, 111),
    );

    let pct = if plan_usd > 0.0 {
        snap.month.cost / plan_usd
    } else {
        0.0
    };
    if plan_usd > 0.0 {
        let pct_label = if pct > 1.0 {
            format!("+{:.0}%", (pct - 1.0) * 100.0)
        } else {
            format!("{:.0}%", pct * 100.0)
        };
        draw_right(ui, &pct_label, [230, 54, 82, 24], rgb(23, 28, 32));
    }
    progress(ui, [18, 118, 294, 8], pct);

    let plan_line = if plan_usd > 0.0 && snap.month.cost > plan_usd {
        format!(
            "{} {} (+{:.0}%)",
            money(snap.month.cost - plan_usd),
            t(lang, "over", "adicional"),
            (pct - 1.0) * 100.0
        )
    } else if plan_usd > 0.0 {
        format!("{:.0}% {}", pct * 100.0, t(lang, "of plan", "del plan"))
    } else {
        plan_limit(plan, lang).to_string()
    };
    draw(ui, &plan_line, [18, 138, 170, 20], rgb(61, 67, 73));
    draw_right(
        ui,
        &format!("{} {}", t(lang, "today", "hoy"), money(snap.today.cost)),
        [190, 138, 122, 20],
        rgb(61, 67, 73),
    );

    let has_total = snap.all_time.is_some();
    if let Some(all_time) = snap.all_time {
        draw(
            ui,
            t(lang, "total", "total"),
            [18, 160, 42, 18],
            rgb(95, 104, 111),
        );
        draw_medium(
            ui,
            &money(all_time.cost),
            [58, 154, 122, 26],
            rgb(23, 28, 32),
        );
        draw_right(
            ui,
            &snap.all_time_updated,
            [190, 160, 122, 18],
            rgb(123, 130, 136),
        );
    }
    let (metrics_y, status_y) = if has_total { (186, 216) } else { (166, 216) };

    draw(
        ui,
        &format!("{} tokens", compact(snap.month.usage.total)),
        [18, metrics_y, 96, 20],
        rgb(95, 104, 111),
    );
    draw(
        ui,
        &format!("{} {}", snap.month.calls, t(lang, "calls", "llamadas")),
        [124, metrics_y, 76, 20],
        rgb(95, 104, 111),
    );
    draw(
        ui,
        &format!(
            "{} {}",
            snap.month.sessions,
            t(lang, "sessions", "sesiones")
        ),
        [208, metrics_y, 104, 20],
        rgb(95, 104, 111),
    );

    let status = snap.error.clone().unwrap_or_else(|| {
        format!(
            "{} - {} {}",
            plan_name(plan, lang),
            t(lang, "updated", "actualizado"),
            snap.updated
        )
    });
    draw(ui, &status, [18, status_y, 294, 18], rgb(123, 130, 136));

    EndPaint(hwnd, &ps);
}

unsafe fn fill(hdc: HDC, rect: RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, &rect, brush);
    DeleteObject(brush);
}

unsafe fn progress(ui: Ui, rect: [i32; 4], pct: f64) {
    let [x, y, w, h] = rect;
    round_fill(ui, rect, 6, rgb(225, 229, 225));
    let fill_w = (w as f64 * pct.clamp(0.0, 1.0)).round() as i32;
    if fill_w > 0 {
        let color = if pct > 1.0 {
            rgb(177, 89, 73)
        } else {
            rgb(44, 129, 103)
        };
        round_fill(ui, [x, y, fill_w, h], 6, color);
    }
}

unsafe fn round_fill(ui: Ui, rect: [i32; 4], radius: i32, color: COLORREF) {
    let [x, y, w, h] = rect.map(|v| z(ui.dpi, v));
    let region = round_region(ui, [x, y, w, h], radius);
    let brush = CreateSolidBrush(color);
    FillRgn(ui.hdc, region, brush);
    DeleteObject(brush);
    DeleteObject(region);
}

unsafe fn round_frame(ui: Ui, rect: [i32; 4], radius: i32, color: COLORREF) {
    let [x, y, w, h] = rect.map(|v| z(ui.dpi, v));
    let region = round_region(ui, [x, y, w, h], radius);
    let brush = CreateSolidBrush(color);
    FrameRgn(ui.hdc, region, brush, z(ui.dpi, 1), z(ui.dpi, 1));
    DeleteObject(brush);
    DeleteObject(region);
}

unsafe fn round_region(ui: Ui, rect: [i32; 4], radius: i32) -> HRGN {
    let [x, y, w, h] = rect;
    let r = z(ui.dpi, radius * 2);
    CreateRoundRectRgn(x, y, x + w, y + h, r, r)
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

unsafe fn draw_big(ui: Ui, text: &str, rect: [i32; 4], color: COLORREF) {
    draw_font(ui, text, rect, color, 30, 600);
}

unsafe fn draw_medium(ui: Ui, text: &str, rect: [i32; 4], color: COLORREF) {
    draw_font(ui, text, rect, color, 17, 600);
}

unsafe fn draw_font(ui: Ui, text: &str, rect: [i32; 4], color: COLORREF, size: i32, weight: i32) {
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
        DT_LEFT,
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
    Config {
        plan,
        monthly_usd_override,
        language,
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

unsafe fn app_icon() -> HICON {
    let icon = LoadImageW(
        GetModuleHandleW(null()),
        std::ptr::without_provenance::<u16>(1),
        IMAGE_ICON,
        0,
        0,
        LR_DEFAULTSIZE | LR_SHARED,
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

    #[test]
    fn price_uses_longest_matching_model_prefix() {
        let price = price_for_model("gpt-5.4-mini-2026-04-28").unwrap();
        assert_eq!(price.input, 0.75);
        assert_eq!(price.output, 4.5);
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
        let period = parse_session(&path, "gpt-5.5", &mut unknown).unwrap();
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
        let period = parse_session(&path, "gpt-5.5", &mut unknown).unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(period.calls, 2);
        assert_eq!(period.usage.input, 108);
        assert_eq!(period.usage.output, 12);
        assert_eq!(period.usage.total, 120);
    }

    #[test]
    fn config_parses_plan_override_and_language() {
        let config = config_from_json(serde_json::json!({
            "plan": "pro_20x",
            "monthly_usd_override": 199.0,
            "language": "es"
        }));
        assert_eq!(current_plan(&config).id, "pro_20x");
        assert_eq!(plan_usd(&config), 199.0);
        assert_eq!(current_lang(&config), Lang::Es);
    }

    #[test]
    fn config_rejects_unknown_values() {
        let config = config_from_json(serde_json::json!({
            "plan": "unknown",
            "monthly_usd_override": -1.0,
            "language": "fr"
        }));
        assert_eq!(current_plan(&config).id, "plus");
        assert_eq!(plan_usd(&config), 20.0);
        assert_eq!(config.language, "auto");
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
    fn all_time_is_on_demand_by_default() {
        assert!(Snapshot::default().all_time.is_none());
    }
}
