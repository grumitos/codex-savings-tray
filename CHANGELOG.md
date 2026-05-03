# Changelog

## 0.2.1 - 2026-05-03

- Account for Codex Fast mode as a pricing multiplier for supported models.
- Detect pricing tier from event metadata when present, then fall back to
  `CODEX_SAVINGS_SERVICE_TIER`, `~/.codex/config.toml`, or `standard`.
- Show the active pricing tier in `--once` output and include Fast mode rates
  in the local credit-rate summary.
- Document the Fast mode estimation behavior, including the API-key standard
  pricing caveat and historical-session fallback.
- Add regression tests for Fast mode pricing, event-level tier precedence, and
  config-file tier parsing.

## 0.1.0 - 2026-04-28

- Initial Rust/MSVC Windows tray app.
- Month-to-date and today API-equivalent usage estimates.
- On-demand all-time scan from local Codex history.
- Native Win32 tray UI without webview or Python runtime.
