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
    Graphics::Gdi::{
        BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
        GetMonitorInfoW, GetStockObject, InvalidateRect, MonitorFromPoint, Rectangle, SelectObject,
        SetBkMode, SetTextColor, DEFAULT_GUI_FONT, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT,
        DT_SINGLELINE, HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST, TRANSPARENT,
    },
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetLocalTime},
    UI::{
        Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
            NOTIFYICONDATAW,
        },
        WindowsAndMessaging::*,
    },
};

const PLAN_USD: f64 = 20.0;
const WM_TRAYICON: u32 = WM_USER + 7;
const TRAY_UID: u32 = 1;
const TIMER_UID: usize = 1;
const ID_REFRESH: usize = 1001;
const ID_EXIT: usize = 1002;
const ID_ALL_TIME: usize = 1003;

#[derive(Clone, Copy)]
struct Price {
    input: f64,
    cached: f64,
    output: f64,
}

const PRICES: &[(&str, Price)] = &[
    (
        "gpt-5.5",
        Price {
            input: 5.0,
            cached: 0.5,
            output: 30.0,
        },
    ),
    (
        "gpt-5.4-mini",
        Price {
            input: 0.75,
            cached: 0.075,
            output: 4.5,
        },
    ),
    (
        "gpt-5.4-nano",
        Price {
            input: 0.20,
            cached: 0.02,
            output: 1.25,
        },
    ),
    (
        "gpt-5.4",
        Price {
            input: 2.5,
            cached: 0.25,
            output: 15.0,
        },
    ),
    (
        "gpt-5.3-codex",
        Price {
            input: 1.75,
            cached: 0.175,
            output: 14.0,
        },
    ),
    (
        "gpt-5.2-codex",
        Price {
            input: 1.75,
            cached: 0.175,
            output: 14.0,
        },
    ),
    (
        "gpt-5.1-codex-max",
        Price {
            input: 1.25,
            cached: 0.125,
            output: 10.0,
        },
    ),
    (
        "gpt-5.1-codex",
        Price {
            input: 1.25,
            cached: 0.125,
            output: 10.0,
        },
    ),
    (
        "gpt-5.1",
        Price {
            input: 1.25,
            cached: 0.125,
            output: 10.0,
        },
    ),
    (
        "gpt-5",
        Price {
            input: 1.25,
            cached: 0.125,
            output: 10.0,
        },
    ),
];

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
    if include_all_time {
        calculate_all_time(&mut snap);
    }
    println!("Codex savings tray");
    println!("Home: {}", snap.codex_home.display());
    println!(
        "Month: {} ({:.0}% of Plus)",
        money(snap.month.cost),
        snap.month.cost / PLAN_USD * 100.0
    );
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
    if snap.assumed_models > 0 {
        println!("Assumed model for {} session(s)", snap.assumed_models);
    }
    if !snap.unknown_models.is_empty() {
        println!("Unknown price models: {}", snap.unknown_models.join(", "));
    }
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
    SNAPSHOT.set(Mutex::new(Snapshot::default())).ok();
    let instance = GetModuleHandleW(null());
    let class = wide("CodexSavingsTray");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        style: CS_HREDRAW | CS_VREDRAW,
        ..zeroed()
    };
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class.as_ptr(),
        wide("Codex Savings").as_ptr(),
        WS_POPUP | WS_BORDER,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        336,
        218,
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
            match wparam & 0xffff {
                ID_REFRESH => refresh(hwnd),
                ID_ALL_TIME => refresh_all_time(hwnd),
                ID_EXIT => {
                    DestroyWindow(hwnd);
                }
                _ => {}
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
    data.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);

    let tip = SNAPSHOT
        .get()
        .and_then(|s| {
            s.lock()
                .ok()
                .map(|s| format!("Codex: {} this month", money(s.month.cost)))
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
    let w = 336;
    let h = 218;
    let x = (pt.x - w + 20).clamp(info.rcWork.left, info.rcWork.right - w);
    let y = (pt.y - h - 12).clamp(info.rcWork.top, info.rcWork.bottom - h);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    SetForegroundWindow(hwnd);
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING, ID_REFRESH, wide("Refresh").as_ptr());
    AppendMenuW(
        menu,
        MF_STRING,
        ID_ALL_TIME,
        wide("All-time total").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_EXIT, wide("Exit").as_ptr());
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
    let mut ps = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rect = zeroed();
    GetClientRect(hwnd, &mut rect);

    fill(hdc, rect, rgb(248, 249, 247));
    SetBkMode(hdc, TRANSPARENT as i32);
    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));

    draw(hdc, "Codex savings", 18, 14, 300, 20, rgb(61, 67, 73));
    let headline = money(snap.month.cost);
    draw_big(hdc, &headline, 18, 42, 180, 42, rgb(23, 28, 32));
    draw(
        hdc,
        "month API equivalent",
        18,
        84,
        180,
        20,
        rgb(95, 104, 111),
    );

    let pct = if PLAN_USD > 0.0 {
        snap.month.cost / PLAN_USD
    } else {
        0.0
    };
    draw_right(
        hdc,
        &format!("{:.0}%", pct * 100.0),
        230,
        50,
        82,
        24,
        rgb(23, 28, 32),
    );
    progress(hdc, 18, 112, 294, 10, pct);

    let net = snap.month.cost - PLAN_USD;
    let net_label = if net >= 0.0 { "over Plus" } else { "to Plus" };
    draw(
        hdc,
        &format!("{} {}", money(net.abs()), net_label),
        18,
        132,
        140,
        20,
        rgb(61, 67, 73),
    );
    draw_right(
        hdc,
        &format!("today {}", money(snap.today.cost)),
        170,
        132,
        142,
        20,
        rgb(61, 67, 73),
    );

    if let Some(all_time) = snap.all_time {
        draw(
            hdc,
            &format!("all-time {}", money(all_time.cost)),
            18,
            148,
            160,
            18,
            rgb(61, 67, 73),
        );
        draw_right(
            hdc,
            &format!("at {}", snap.all_time_updated),
            190,
            148,
            122,
            18,
            rgb(123, 130, 136),
        );
    }

    draw(
        hdc,
        &format!("{} tokens", compact(snap.month.usage.total)),
        18,
        164,
        96,
        20,
        rgb(95, 104, 111),
    );
    draw(
        hdc,
        &format!("{} calls", snap.month.calls),
        124,
        164,
        76,
        20,
        rgb(95, 104, 111),
    );
    draw(
        hdc,
        &format!("{} sessions", snap.month.sessions),
        208,
        164,
        104,
        20,
        rgb(95, 104, 111),
    );

    let status = snap
        .error
        .clone()
        .or_else(|| (!snap.unknown_models.is_empty()).then(|| "unknown model price".to_string()))
        .or_else(|| (snap.assumed_models > 0).then(|| "using fallback model".to_string()))
        .unwrap_or_else(|| format!("updated {}", snap.updated));
    draw(hdc, &status, 18, 190, 294, 18, rgb(123, 130, 136));

    EndPaint(hwnd, &ps);
}

unsafe fn fill(hdc: HDC, rect: RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, &rect, brush);
    DeleteObject(brush);
}

unsafe fn progress(hdc: HDC, x: i32, y: i32, w: i32, h: i32, pct: f64) {
    let track = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    fill(hdc, track, rgb(225, 229, 225));
    let fill_w = ((w as f64 * pct.min(1.0)).round() as i32).max(2);
    let bar = RECT {
        right: x + fill_w,
        ..track
    };
    fill(hdc, bar, rgb(44, 129, 103));
    Rectangle(hdc, x, y, x + w, y + h);
}

unsafe fn draw(hdc: HDC, text: &str, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
    draw_text(
        hdc,
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

unsafe fn draw_right(hdc: HDC, text: &str, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
    draw_text(
        hdc,
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

unsafe fn draw_big(hdc: HDC, text: &str, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
    let name = wide("Segoe UI");
    let font = CreateFontW(-30, 0, 0, 0, 600, 0, 0, 0, 0, 0, 0, 5, 0, name.as_ptr());
    let old = SelectObject(hdc, font);
    draw_text(
        hdc,
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

unsafe fn draw_text(hdc: HDC, text: &str, mut rect: RECT, color: COLORREF, align: u32) {
    let text = wide(text);
    SetTextColor(hdc, color);
    DrawTextW(
        hdc,
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
}
