# Codex Savings Tray

Tiny Windows tray app that estimates the API-equivalent value of local Codex
usage and compares the month-to-date total with your selected ChatGPT/Codex
plan.

It reads local Codex CLI/App data only:

- `~/.codex/sessions/**/*.jsonl` for cumulative token usage emitted by Codex
  CLI/App `token_count` events.
- `~/.codex/state_5.sqlite` for the model attached to each rollout.

It does not read `auth.json`, copy tokens, use API keys, make network calls, or
proxy requests.

## Use

Run the release executable:

```powershell
.\target\release\codex-savings-tray.exe
```

Tray actions:

- Left click: show or hide the compact view.
- Right click: choose `Plan`, `Reload`, `Plan start day`,
  `Calculate total saved`, `Open config`, `Open usage dashboard`, or `Exit`.
- Background refresh: month-to-date only, every 5 minutes.
- All-time total: scanned only when selected, so old history does not cost
  resources during normal use.

Config lives at:

```text
%APPDATA%\Codex Savings Tracker\config.json
```

Supported config values:

```json
{
  "plan": "plus",
  "monthly_usd_override": null,
  "language": "auto",
  "cycle_day": 1
}
```

`plan` can be `free`, `go`, `plus`, `pro_5x`, `pro_20x`, `business`,
`enterprise_edu`, `api_key`, or `custom`. `language` can be `auto`, `en`, or
`es`. `cycle_day` is the recurring monthly plan start day, from `1` to `31`.
The app follows the common subscription anchor behavior used by services such
as OpenAI and Stripe: renew on the same day of the month when possible, and use
the month's last day when that day does not exist.

Diagnostic CLI:

```powershell
.\target\release\codex-savings-tray.exe --once
.\target\release\codex-savings-tray.exe --once --all-time
```

## Build

This repo is pinned to Rust stable MSVC through `rust-toolchain.toml`.

Builder requirements:

- Rust toolchain: `stable-x86_64-pc-windows-msvc`
- Visual Studio Build Tools 2022 with C++ build tools

Build:

```powershell
cargo build --release
```

Quality checks:

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo audit
```

The user who receives the built `.exe` does not need Rust, Python, MSYS2,
SQLite, or Visual Studio Build Tools. SQLite is embedded through `rusqlite`
with the bundled SQLite feature.

## Release

Create a tag such as `v0.1.0`, build locally, zip the release `.exe`, and
create a GitHub Release.

See `docs/RELEASE.md` for the full checklist.

## Cost Method

For each token usage event, the app reads cumulative `total_token_usage` and
adds only the delta from the previous event. This avoids double-counting repeat
records.

Cost is:

```text
uncached_input = input_tokens - cached_input_tokens
cost =
  uncached_input * input_price
  + cached_input_tokens * cached_input_price
  + output_tokens * output_price
```

`reasoning_output_tokens` is shown as detail inside token totals but is not
billed again because Codex total tokens equal input plus output.

## Built-in Prices

Prices are USD per 1M tokens:

```text
gpt-5.5         input 5.00   cached 0.50    output 30.00
gpt-5.4         input 2.50   cached 0.25    output 15.00
gpt-5.4-mini    input 0.75   cached 0.075   output 4.50
gpt-5.3-codex   input 1.75   cached 0.175   output 14.00
gpt-5.2-codex   input 1.75   cached 0.175   output 14.00
gpt-5.1-codex   input 1.25   cached 0.125   output 10.00
```

Set `CODEX_SAVINGS_MODEL` to choose a fallback model if SQLite metadata is not
available. The default fallback is `gpt-5.5`.
