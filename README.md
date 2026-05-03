# Codex Savings Tray

Tiny Windows tray app that estimates the API-equivalent value of local Codex
usage and compares the month-to-date total with your selected ChatGPT/Codex
plan.

It reads local Codex CLI/App data only:

- `~/.codex/sessions/**/*.jsonl` for cumulative token usage emitted by Codex
  CLI/App `token_count` events.
- `~/.codex/state_5.sqlite` for the model attached to each rollout.
- `~/.codex/config.toml` for the default `service_tier` when a session event
  does not include tier metadata.

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
- Single instance: running the `.exe` again replaces the existing tray process.
  The new process asks the previous one to close cleanly; if the previous tray
  window is hung, it terminates that process before opening. If the app crashed,
  Windows releases the instance lock and the next launch starts normally.

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

The diagnostic output includes `Pricing tier: standard` or `Pricing tier: fast`
so the estimate makes the active multiplier visible.

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

Create a tag such as `v0.2.1`, build locally, generate the versioned `.exe` and
`.sha256` assets, and create a GitHub Release.

See `docs/RELEASE.md` for the full checklist.

## Cost Method

For each token usage event, the app reads cumulative `total_token_usage` and
adds only the delta from the previous event. This avoids double-counting repeat
records.

The current subscription cycle is split by each event's timestamp, converted to
local date, not by the session file date alone. A session started before the
plan start day and continued after it only contributes the in-cycle deltas. If
the first visible in-cycle event already includes earlier accumulated totals,
the app uses `last_token_usage` when Codex records it so earlier work is not
charged to the current cycle.

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

Fast mode is applied as a price multiplier, not as extra tokens. The app first
uses `service_tier` or `speed` if Codex records it on the usage event. If not,
it falls back to `CODEX_SAVINGS_SERVICE_TIER`, then `~/.codex/config.toml`, then
`standard`. For historical sessions without event-level tier metadata, the
configured tier is an estimate.

Codex API-key usage should stay on `standard` pricing because Fast mode credits
apply to ChatGPT-signed-in Codex usage, not API-key billing.

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

Fast mode multipliers are applied to supported models only:

```text
gpt-5.5 fast     x2.5 multiplier
gpt-5.4 fast     x2.0 multiplier
```

Effective Fast mode prices are:

```text
gpt-5.5 fast     input 12.50  cached 1.25   output 75.00
gpt-5.4 fast     input 5.00   cached 0.50   output 30.00
```

Set `CODEX_SAVINGS_MODEL` to choose a fallback model if SQLite metadata is not
available. Set `CODEX_SAVINGS_SERVICE_TIER` to choose a fallback pricing tier
when event metadata is missing. The defaults are `gpt-5.5` and `standard`.
