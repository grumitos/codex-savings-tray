# Rust core FFI contract

The WinUI shell calls `codex_savings_core.dll` with the C ABI. Every response
is a UTF-8 JSON string allocated by Rust and must be released exactly once with
`cst_free_string`.

| Function | Input | Success data |
| --- | --- | --- |
| `cst_scan_current` | none | current-cycle SnapshotDto |
| `cst_scan_all_time` | none | SnapshotDto including allTime |
| `cst_load_settings` | none | normalized ConfigDto and plans |
| `cst_preview_cycle` | UTF-8 ConfigDto JSON | next cycle date and days remaining, without persistence |
| `cst_save_settings` | UTF-8 ConfigDto JSON | normalized ConfigDto |
| `cst_free_string` | a prior result pointer | none |

Responses have one shape:

```json
{ "ok": true, "data": {} }
```

or:

```json
{ "ok": false, "error": { "code": "invalid_cycle_day", "message": "..." } }
```

The FFI uses camelCase DTO fields. The persisted configuration remains
`%APPDATA%\\Codex Savings Tracker\\config.json` with its existing snake_case
schema for compatibility.

Settings are validated at the FFI boundary: supported plans, `auto|en|es`,
cycle day `1..31`, and custom amount `0..1,000,000` USD. Callers must pass a
valid, NUL-terminated UTF-8 string to `cst_save_settings` and must only free
unmodified pointers returned by this library.
