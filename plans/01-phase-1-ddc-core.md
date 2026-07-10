# Phase 1 — DDC Core

**Goal:** a working `ddc` module that enumerates monitors, gives each a stable cross-machine identity,
reads/writes VCP `0x60` (input source), and decodes the advertised input list — plus a `--cli` harness
to exercise it by hand. No GUI, no networking yet.

**Prerequisite reading:** `00-overview.md` (esp. the VCP code table and "verify crate APIs" note).

## Deliverables

- `src/ddc/mod.rs`, `src/ddc/identity.rs`, `src/ddc/inputs.rs`
- `src/logging.rs`
- `src/main.rs` wired so `mplexer --cli <subcommand>` runs the harness (GUI comes in phase 2; until
  then, `main` with no args may just print "GUI not yet implemented").
- Unit tests for the pure functions in `inputs.rs` and `identity.rs`.

## Dependencies to add

```bash
cargo add ddc-hi
cargo add clap --features derive
cargo add log env_logger
cargo add anyhow            # error handling in the ddc layer + cli
```

Record the resolved versions and confirm the `ddc-hi` API on docs.rs before writing against it.

## `ddc-hi` API orientation (verify against resolved version)

`ddc-hi` exposes a `Display` type and a `Query`/enumeration entry point. The shape is roughly:

- `ddc_hi::Display::enumerate() -> Vec<Display>` — all detected displays.
- `Display::update_capabilities()` — populate the capability info (may be needed before `.info` is
  fully filled and before you can trust the VCP value list). This performs I/O; it can be slow/failing
  on some backends — handle errors, don't unwrap.
- `Display::info` — a `DisplayInfo` with fields such as `manufacturer_id`, `model_name`,
  `serial_number` (string), `serial` (numeric), `model_id`. Availability varies by backend/monitor.
- The `ddc::Ddc` trait (re-exported by `ddc-hi`, implemented by `Display.handle` or `Display` itself
  depending on version) provides:
  - `get_vcp_feature(code: u8) -> Result<VcpValue>` where `VcpValue { value: u16, maximum: u16, .. }`
    (field names vary — check docs). For `0x60`, `value` is the current input code.
  - `set_vcp_feature(code: u8, value: u16) -> Result<()>`.
  - `capabilities_string() -> Result<Vec<u8>>` (raw caps bytes) — may live on `Ddc` or a companion
    trait. The raw string contains a `vcp(...)` section listing supported feature codes and, for
    `60(...)`, the valid input values in hex.

> If `capabilities_string()` is awkward or unavailable via `ddc-hi`, you can still obtain the valid
> input list two ways: (a) parse `Display::info` capability data if the crate already parsed it, or
> (b) fall back to a small built-in candidate set and probe. Prefer parsing the caps string; document
> whatever you choose.

## `src/ddc/inputs.rs` — input-source model (pure, unit-tested)

```rust
/// A VCP 0x60 input-source value with a human label.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputSource(pub u8);

impl InputSource {
    /// Standard MCCS label, or `Input 0xNN` for vendor/unknown codes.
    pub fn label(self) -> String { /* match on the table in 00-overview.md */ }
}

/// Parse the `60(...)` values out of a raw DDC capabilities string.
/// Example fragment: `... vcp(02 04 ... 60(11 12 0F) ...) ...`
/// Returns the advertised input codes in the order listed.
pub fn parse_input_list(caps: &str) -> Vec<InputSource> { /* ... */ }
```

Implementation notes:

- `label`: match the table in the overview; unknown → `format!("Input 0x{:02X}", self.0)`.
- `parse_input_list`: locate `vcp(`, then within it find the `60(` group and read space-separated hex
  bytes until the matching `)`. Be tolerant of extra whitespace and upper/lowercase. If no `60(` group
  exists, return an empty vec (caller falls back).

**Unit tests (no hardware):**

- `parse_input_list("(prot(monitor)type(lcd)... vcp(02 04 60(11 12 0F) 62 ...) ...)")`
  → `[0x11, 0x12, 0x0F]`.
- Caps string without a `60(` group → `[]`.
- `InputSource(0x11).label() == "HDMI-1"`; `InputSource(0x12).label() == "HDMI-2"`;
  `InputSource(0x0F).label() == "DisplayPort-1"`; `InputSource(0xAB).label() == "Input 0xAB"`.

## `src/ddc/identity.rs` — stable identity (pure, unit-tested)

The same physical monitor must produce the same `StableId` on both machines, even though each machine
reads it over a different input. Manufacturers normally replicate EDID (model + serial) across a
monitor's inputs, so build the id from EDID-derived fields, **not** from OS handle or index.

```rust
#[derive(Clone, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct StableId(String);

/// Build from whatever identifying fields the backend provides.
/// Priority: serial_number (string) > numeric serial > model_name+model_id.
/// Always include manufacturer + model so different panels of the same model with no serial
/// still differ when possible. Fall back to a provided index only as a last resort (log a warning).
pub fn stable_id(
    manufacturer: Option<&str>,
    model_name: Option<&str>,
    model_id: Option<u16>,
    serial_number: Option<&str>,
    serial: Option<u32>,
    fallback_index: usize,
) -> StableId { /* deterministic string join, normalized (trim, lowercase) */ }
```

**Gotcha to document in code:** if two identical monitors expose no serial, their `StableId` will
collide. Note this; phase 3 can additionally disambiguate by position/order if needed, but keep phase 1
simple.

**Unit tests:** same inputs → equal ids; differing serials → different ids; missing serial falls back
to model fields deterministically.

## `src/ddc/mod.rs` — public API

```rust
pub mod identity;
pub mod inputs;

pub use identity::StableId;
pub use inputs::InputSource;

pub struct Monitor {
    pub id: StableId,
    pub name: String,             // friendly, e.g. model_name or "Monitor 1"
    pub current_input: Option<InputSource>,
    pub valid_inputs: Vec<InputSource>,
    handle: ddc_hi::Display,      // keep the backend handle for read/write
}

/// Enumerate all locally-attached DDC/CI monitors. Best-effort: skip and log any display whose
/// capabilities/VCP can't be read, rather than failing the whole call.
pub fn enumerate() -> Vec<Monitor>;

impl Monitor {
    /// Read VCP 0x60 fresh from the hardware.
    pub fn read_input(&mut self) -> anyhow::Result<InputSource>;
    /// Write VCP 0x60. Caller is responsible for only doing this when it owns the monitor.
    pub fn write_input(&mut self, input: InputSource) -> anyhow::Result<()>;
}
```

Implementation notes:

- In `enumerate`: for each `ddc_hi::Display`, call `update_capabilities()`, build the `StableId` from
  `info`, read current VCP `0x60` (tolerate failure → `current_input = None`), parse valid inputs from
  the caps string (fall back to `[current]` if empty). Wrap per-display work in a `Result` and
  `continue` on error with a `log::warn!`.
- `read_input`/`write_input`: thin wrappers over the `Ddc` trait calls; map errors via `anyhow` with
  context (`.with_context(|| format!("VCP 0x60 on {}", self.name))`).
- Keep `handle` private. `Monitor` is not `Clone` (holds a live handle).

## `src/logging.rs`

```rust
pub fn init() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    ).init();
}
```

## `src/main.rs` — CLI harness

Use `clap` derive. Default (no `--cli`) → phase-2 GUI later; for now print a placeholder. `--cli`
exposes:

```
mplexer --cli list                     # enumerate; print id, name, current input, valid inputs
mplexer --cli get   <id>               # read + print current input for one monitor
mplexer --cli set   <id> <hex_code>    # write input (e.g. 0x11). GUARDED: see below.
```

- `list` output example (one line per monitor):
  `#0  id=DELL-U2419H-ABC123  name="DELL U2419H"  current=HDMI-2(0x12)  valid=[HDMI-1, HDMI-2, DisplayPort-1]`
- `set` must print a **loud warning** and require an explicit `--yes` flag to actually write, because a
  wrong switch can blank the operator's screen:
  `mplexer --cli set <id> 0x11 --yes`. Without `--yes`, print what it *would* do and exit.
- Accept the monitor id as the full `StableId` string or a `#index` from `list` for convenience.

## Acceptance criteria

Automated (run here, no hardware):

- `cargo build` and `cargo clippy --all-targets` are clean.
- `cargo test` passes (the `inputs.rs` and `identity.rs` unit tests above).

Hardware validation (commands for the **user** to run; include them in the phase-1 completion note):

- Windows: `mplexer --cli list` lists both monitors with `current=HDMI-2(0x12)` and
  `valid=[HDMI-1, HDMI-2, DisplayPort-1]`.
- Fedora: after `sudo modprobe i2c-dev` and adding the user to the `i2c` group,
  `mplexer --cli list` lists the same two monitors with matching `StableId`s. (This is the NVIDIA-Linux
  risk checkpoint — if enumeration is empty, cross-check with `ddcutil detect`.)
- A **user-supervised** `mplexer --cli set <id> 0x11 --yes` on one monitor switches it; running `set`
  again with `0x12` restores it. Only the user runs these, watching the screen.

## Definition of done

`ddc::enumerate()` returns correctly-identified monitors with decoded current + valid inputs on both
OSes; read/write works under supervision; pure functions are unit-tested; clippy clean.
