use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::{c_char, CStr, CString},
    fs::{self, File},
    io::{BufRead, BufReader},
    mem::zeroed,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    ptr::null,
    sync::{Mutex, OnceLock},
    time::SystemTime,
};
use windows_sys::Win32::{
    Foundation::{FILETIME, SYSTEMTIME},
    System::{
        SystemInformation::GetLocalTime,
        Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
    },
};

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

static CURRENT_CACHE: OnceLock<Mutex<HashMap<PathBuf, CurrentCacheEntry>>> = OnceLock::new();

/// Executes the development-only diagnostic CLI.
pub fn run_cli(include_all_time: bool) {
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

fn local_time() -> SYSTEMTIME {
    unsafe {
        let mut time = zeroed();
        GetLocalTime(&mut time);
        time
    }
}

fn money(value: f64) -> String {
    format!("${value:.2}")
}

fn compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
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
    let _ = save_config(config);
}

fn save_config(config: &Config) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the settings directory: {error}"))?;
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
    let text = serde_json::to_string_pretty(&text)
        .map_err(|error| format!("Could not serialize settings: {error}"))?;
    fs::write(path, text).map_err(|error| format!("Could not save settings: {error}"))
}

fn config_path() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Codex Savings Tracker")
        .join("config.json")
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

type FfiError = (&'static str, String);

fn config_from_ffi_json(input: &str) -> Result<Config, FfiError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|_| ("invalid_json", "Settings must be valid JSON.".to_string()))?;
    let object = value.as_object().ok_or((
        "invalid_settings",
        "Settings must be a JSON object.".to_string(),
    ))?;

    let plan = object
        .get("plan")
        .and_then(Value::as_str)
        .filter(|id| plan_by_id(id).is_some())
        .ok_or(("invalid_plan", "Choose a supported plan.".to_string()))?;
    let language = object
        .get("language")
        .and_then(Value::as_str)
        .filter(|language| matches!(*language, "auto" | "en" | "es"))
        .ok_or((
            "invalid_language",
            "Language must be auto, en, or es.".to_string(),
        ))?;
    let cycle_day = object
        .get("cycleDay")
        .and_then(Value::as_u64)
        .filter(|day| (1..=31).contains(day))
        .map(|day| day as u16)
        .ok_or((
            "invalid_cycle_day",
            "Cycle day must be an integer from 1 to 31.".to_string(),
        ))?;
    let monthly_usd_override = match object.get("monthlyUsdOverride") {
        None | Some(Value::Null) => None,
        Some(value) => value
            .as_f64()
            .filter(|amount| amount.is_finite() && (0.0..=1_000_000.0).contains(amount))
            .ok_or((
                "invalid_amount",
                "Custom amount must be between 0 and 1,000,000 USD.".to_string(),
            ))
            .map(Some)?,
    };
    if plan == "custom" && monthly_usd_override.is_none() {
        return Err((
            "invalid_amount",
            "A custom plan requires a monthly amount.".to_string(),
        ));
    }

    Ok(Config {
        plan: plan.to_string(),
        monthly_usd_override,
        language: language.to_string(),
        cycle_day,
    })
}

unsafe fn config_from_ffi_pointer(json: *const c_char) -> Result<Config, FfiError> {
    if json.is_null() {
        return Err(("invalid_json", "Settings JSON is required.".to_string()));
    }
    let input = unsafe { CStr::from_ptr(json) }
        .to_str()
        .map_err(|_| ("invalid_json", "Settings must be UTF-8 JSON.".to_string()))?;
    config_from_ffi_json(input)
}

fn config_json(config: &Config) -> Value {
    serde_json::json!({
        "plan": config.plan,
        "monthlyUsdOverride": config.monthly_usd_override,
        "language": config.language,
        "cycleDay": config.cycle_day,
    })
}

fn period_json(period: &Period) -> Value {
    serde_json::json!({
        "costUsd": period.cost,
        "calls": period.calls,
        "sessions": period.sessions,
        "inputTokens": period.usage.input,
        "cachedInputTokens": period.usage.cached,
        "cacheWriteTokens": period.usage.cache_write,
        "outputTokens": period.usage.output,
        "reasoningOutputTokens": period.usage.reasoning,
        "totalTokens": period.usage.total,
    })
}

fn snapshot_json(snapshot: &Snapshot) -> Value {
    serde_json::json!({
        "cycle": period_json(&snapshot.month),
        "today": period_json(&snapshot.today),
        "allTime": snapshot.all_time.as_ref().map(period_json),
        "config": config_json(&snapshot.config),
        "serviceTier": service_tier_label(snapshot.service_tier),
        "cycleStart": format_date(snapshot.cycle_start),
        "cycleNext": format_date(snapshot.cycle_next),
        "daysUntilReset": snapshot.cycle_days_left,
        "updatedAt": snapshot.updated,
        "allTimeUpdatedAt": snapshot.all_time_updated,
        "codexHome": snapshot.codex_home,
        "unknownModels": snapshot.unknown_models,
        "assumedModels": snapshot.assumed_models,
        "error": snapshot.error,
    })
}

fn settings_json(config: &Config) -> Value {
    serde_json::json!({
        "config": config_json(config),
        "plans": PLANS.iter().map(|plan| serde_json::json!({
            "id": plan.id,
            "nameEn": plan.en,
            "nameEs": plan.es,
            "usd": plan.usd,
            "limitsEn": plan.limits_en,
            "limitsEs": plan.limits_es,
        })).collect::<Vec<_>>(),
    })
}

fn cycle_preview_json(config: &Config, today: DateKey) -> Value {
    let cycle_next = next_cycle_start(today, config.cycle_day);
    serde_json::json!({
        "cycleNext": format_date(cycle_next),
        "daysUntilReset": days_between(today, cycle_next).max(0),
    })
}

fn cst_json(value: Value) -> *mut c_char {
    CString::new(value.to_string())
        .expect("JSON cannot contain an interior NUL")
        .into_raw()
}

fn cst_ok(data: Value) -> *mut c_char {
    cst_json(serde_json::json!({ "ok": true, "data": data }))
}

fn cst_error(code: &'static str, message: impl Into<String>) -> *mut c_char {
    cst_json(serde_json::json!({
        "ok": false,
        "error": { "code": code, "message": message.into() },
    }))
}

fn ffi_call(call: impl FnOnce() -> Result<Value, FfiError>) -> *mut c_char {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(Ok(data)) => cst_ok(data),
        Ok(Err((code, message))) => cst_error(code, message),
        Err(_) => cst_error("internal_error", "The core could not complete the request."),
    }
}

#[no_mangle]
pub extern "C" fn cst_scan_current() -> *mut c_char {
    ffi_call(|| {
        let config = load_config();
        scan_month(&config)
            .map(|snapshot| snapshot_json(&snapshot))
            .map_err(|message| ("scan_failed", message))
    })
}

#[no_mangle]
pub extern "C" fn cst_scan_all_time() -> *mut c_char {
    ffi_call(|| {
        let config = load_config();
        let mut snapshot = scan_month(&config).map_err(|message| ("scan_failed", message))?;
        calculate_all_time(&mut snapshot);
        Ok(snapshot_json(&snapshot))
    })
}

#[no_mangle]
pub extern "C" fn cst_load_settings() -> *mut c_char {
    ffi_call(|| Ok(settings_json(&load_config())))
}

#[no_mangle]
/// Validates UTF-8 JSON settings and previews their next cycle without persisting them.
///
/// # Safety
///
/// `json` must be null or point to a NUL-terminated UTF-8 byte sequence that is
/// readable for the duration of this call.
pub unsafe extern "C" fn cst_preview_cycle(json: *const c_char) -> *mut c_char {
    ffi_call(|| {
        let config = unsafe { config_from_ffi_pointer(json) }?;
        let today = date_from_system(local_time());
        Ok(cycle_preview_json(&config, today))
    })
}

#[no_mangle]
/// Validates and persists UTF-8 JSON settings from a caller-owned C string.
///
/// # Safety
///
/// `json` must be null or point to a NUL-terminated UTF-8 byte sequence that is
/// readable for the duration of this call.
pub unsafe extern "C" fn cst_save_settings(json: *const c_char) -> *mut c_char {
    ffi_call(|| {
        let config = unsafe { config_from_ffi_pointer(json) }?;
        save_config(&config).map_err(|message| ("save_failed", message))?;
        Ok(config_json(&config))
    })
}

#[no_mangle]
/// Releases a string returned by this library.
///
/// # Safety
///
/// `pointer` must be null or an unmodified pointer returned by a `cst_*`
/// function that has not already been released.
pub unsafe extern "C" fn cst_free_string(pointer: *mut c_char) {
    if !pointer.is_null() {
        drop(unsafe { CString::from_raw(pointer) });
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
    fn all_time_is_on_demand_by_default() {
        assert!(Snapshot::default().all_time.is_none());
    }

    #[test]
    fn ffi_rejects_invalid_json() {
        assert_eq!(config_from_ffi_json("{").err().unwrap().0, "invalid_json");
    }

    #[test]
    fn ffi_rejects_unknown_plans() {
        assert_eq!(
            config_from_ffi_json(r#"{"plan":"unknown","language":"en","cycleDay":1}"#)
                .err()
                .unwrap()
                .0,
            "invalid_plan"
        );
    }

    #[test]
    fn ffi_rejects_out_of_range_values() {
        assert_eq!(
            config_from_ffi_json(
                r#"{"plan":"custom","monthlyUsdOverride":1000000.01,"language":"en","cycleDay":31}"#
            )
            .err()
            .unwrap()
            .0,
            "invalid_amount"
        );
    }

    #[test]
    fn ffi_rejects_invalid_cycle_days() {
        assert_eq!(
            config_from_ffi_json(
                r#"{"plan":"plus","monthlyUsdOverride":null,"language":"en","cycleDay":32}"#
            )
            .err()
            .unwrap()
            .0,
            "invalid_cycle_day"
        );
    }

    #[test]
    fn cycle_preview_uses_the_rust_month_clamping_rules() {
        let config = Config {
            cycle_day: 31,
            ..Config::default()
        };
        let preview = cycle_preview_json(
            &config,
            DateKey {
                year: 2026,
                month: 4,
                day: 10,
            },
        );
        assert_eq!(preview["cycleNext"], "2026-04-30");
        assert_eq!(preview["daysUntilReset"], 20);
    }

    #[test]
    fn ffi_responses_are_owned_and_freed() {
        let response = cst_ok(serde_json::json!({"value":"ok"}));
        unsafe {
            let json: Value =
                serde_json::from_str(std::ffi::CStr::from_ptr(response).to_str().unwrap()).unwrap();
            assert_eq!(json["ok"], true);
            assert_eq!(json["data"]["value"], "ok");
            cst_free_string(response);
        }
    }
}
