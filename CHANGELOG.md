# Changelog

## 0.2.1 - 2026-05-03

- Account for Codex Fast mode as a pricing multiplier for supported models.
- Detect pricing tier from event metadata when present, then fall back to
  `CODEX_SAVINGS_SERVICE_TIER`, `~/.codex/config.toml`, or `standard`.
- Show the active pricing tier in `--once` output and include Fast mode rates
  in the local credit-rate summary.
- Document the Fast mode estimation behavior, including the API-key standard
  pricing caveat, historical-session fallback, explicit multipliers, and
  effective Fast mode prices.
- Refresh the manual release checklist with the current versioned asset flow.
- Require bilingual English and Spanish release notes for future releases.
- Add regression tests for Fast mode pricing, event-level tier precedence, and
  config-file tier parsing.
- Split current-cycle and today totals by usage event timestamp, so sessions
  started before the subscription day do not drag prior-cycle usage into the
  current month.
- Use `last_token_usage` for the first in-cycle event when available to avoid
  counting accumulated usage from before the cycle boundary.
- Shrink long popup numbers to fit instead of truncating high percentages or
  large dollar amounts.
- Enforce a single tray process. Relaunching the `.exe` replaces the existing
  process, and a hung previous tray window is terminated before the new process
  opens.

## 0.1.0 - 2026-04-28

- Initial Rust/MSVC Windows tray app.
- Month-to-date and today API-equivalent usage estimates.
- On-demand all-time scan from local Codex history.
- Native Win32 tray UI without webview or Python runtime.
