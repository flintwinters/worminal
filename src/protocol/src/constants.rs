//! Wire constants. Message IDs and feature bits are fixed by installed peers: append, never renumber.
use std::ffi::CStr;

pub const WIRE_VERSION: u32 = 2;
pub const WORKSPACE_ENDPOINT: &CStr = c"worminal-workspace-v2";
pub const WORKSPACE_DAEMON: &CStr = c"worminald";
pub const WIRE_MAX_BYTES: u32 = 8 * 1024 * 1024;
pub const WORKSPACE_MAX_COLS: u32 = 400;
pub const WORKSPACE_MAX_ROWS: u32 = 200;
pub const WORKSPACE_MAX_ARGS: u32 = 1024;
pub const WORKSPACE_MAX_ENV: u32 = 8192;
pub const WORKSPACE_INITIAL_CURSOR: u32 = 2;
/// Rune, attributes, foreground, background: independent of native cell layout.
pub const WIRE_CELL_BYTES: u32 = 4 * 4;

// Service bounds and deadlines.
pub const WORKSPACE_CLIENT_QUEUE_BYTES: u32 = 4 * 1024 * 1024;
pub const WORKSPACE_INPUT_QUEUE_BYTES: u32 = 1024 * 1024;
/// Coalesce engine damage early; queued complete frames and their baselines remain intact.
pub const WORKSPACE_FRAME_BACKLOG_BYTES: u32 = 128 * 1024;
pub const WORKSPACE_RESYNC_BACKLOG_BYTES: u32 = 65536;
pub const WORKSPACE_IO_BYTES: u32 = 16384;
// Minimum spacing of coalesced output frames (30 per second). A terminal gains nothing visible
// from display-rate streaming, and every frame costs remote bandwidth. Input echo bypasses this:
// the first output after delivered input, and scroll/focus replies, are sent immediately.
pub const WORKSPACE_FRAME_INTERVAL_NS: u32 = 33_000_000;
pub const WORKSPACE_DIRECTORY_INTERVAL_NS: u32 = 250_000_000;

pub const WIRE_HELLO: u32 = 1;
pub const WIRE_NEW: u32 = 2;
pub const WIRE_INPUT: u32 = 3;
pub const WIRE_FOCUS: u32 = 4;
pub const WIRE_CLOSE: u32 = 5;
pub const WIRE_SCROLL: u32 = 6;
pub const WIRE_CATALOG: u32 = 7;
pub const WIRE_FRAME: u32 = 8;
pub const WIRE_ROW: u32 = 9;
pub const WIRE_FRAME_FINISH: u32 = 10;
pub const WIRE_CLIPBOARD: u32 = 11;
pub const WIRE_BELL: u32 = 12;
pub const WIRE_STOP: u32 = 13;
pub const WIRE_PRINT: u32 = 14;
pub const WIRE_RELEASE: u32 = 15;
pub const WIRE_ERROR: u32 = 16;
pub const WIRE_BROWSER: u32 = 17;
pub const WIRE_BROWSER_NAVIGATE: u32 = 18;
pub const WIRE_ATTACH: u32 = 19;
pub const WIRE_CREATED: u32 = 20;
pub const WIRE_FOCUS_STATE: u32 = 21;
pub const WIRE_CONTROL: u32 = 22;
pub const WIRE_RESIZE: u32 = 23;
pub const WIRE_BROWSER_REPLACE: u32 = 24;
pub const WIRE_COMMAND_DONE: u32 = 25;
pub const WIRE_HANDIN: u32 = 26;
pub const WIRE_HANDOFF: u32 = 27;
pub const WIRE_CAPABILITIES: u32 = 28;
pub const WIRE_COPY_ROWS: u32 = 29;
pub const WIRE_SERVICE_INFO: u32 = 30;
/// Generation, start/next/cancel; reply: generation, done, UTF-8 bytes.
pub const WIRE_SELECT_ALL: u32 = 31;
/// Row, first column, then one or more complete wire cells.
pub const WIRE_ROW_SPAN: u32 = 32;

/// Automation request: operation u32 followed by optional input bytes. Reply: success u32 and UTF-8 JSON.
pub const WIRE_WORP: u32 = 33;
/// Row, then zero or more link runs (column, count, URI) replacing that row's links; zero runs clear them.
pub const WIRE_ROW_LINKS: u32 = 34;
pub const WORP_LIST: u32 = 0;
pub const WORP_READ: u32 = 1;
pub const WORP_INPUT: u32 = 2;
pub const WORP_DEVICES: u32 = 3;
pub const WORP_ENROLL: u32 = 4;
pub const WORP_FORGET: u32 = 5;
pub const WORP_OPEN_TERMINAL: u32 = 6;
pub const WORP_OPEN_BROWSER: u32 = 7;
pub const WORP_CLOSE: u32 = 8;

// Advertise before HELLO/ATTACH. Older daemons ignore the optional packet;
// unadvertised clients must continue receiving the original row encoding.
pub const WIRE_FEATURE_COPY_ROWS: u32 = 1 << 0;
// Bit 1 advertised the pre-Rust select-all packet at SERVICE_INFO's current ID.
// Keep it reserved so those installed peers cannot negotiate the wrong message.
pub const WIRE_FEATURE_SELECT_ALL: u32 = 1 << 2;
pub const WIRE_FEATURE_ROW_SPAN: u32 = 1 << 3;
// The client displays only the tab it last focused (or created). The daemon defers frames for its
// other controlled tabs, retaining their baselines, and resumes them with a delta on focus.
pub const WIRE_FEATURE_FOCUSED_FRAMES: u32 = 1 << 4;
// ROW_LINKS packets follow a row's cells. Peers that did not negotiate it receive today's bytes exactly.
pub const WIRE_FEATURE_ROW_LINKS: u32 = 1 << 5;

pub const WIRE_SELECTION_START: u32 = 0;
pub const WIRE_SELECTION_NEXT: u32 = 1;
pub const WIRE_SELECTION_CANCEL: u32 = 2;

/// Complete byte-preserving detached launch in the existing Worp envelope.
pub const WORP_OPEN_DETACHED: u32 = 9;

/// OSC 99 metadata and payload, delivered only to capable controlling views.
pub const WIRE_NOTIFICATION: u32 = 35;
pub const WIRE_FEATURE_NOTIFICATIONS: u32 = 1 << 6;

/// Move the packet's tab to a zero-based position in the daemon's tab order (one u32; clamped to the last
/// position). Older daemons ignore it, and the catalog they keep sending restores the old order.
pub const WIRE_MOVE: u32 = 36;

/// A view that alerts a tab (bell or notification) tells the daemon with an empty-bodied command; the daemon
/// relays `ATTENTION` events (one u32: severity, 0 answered) to the other views, so every window's tab strip
/// fades the tab until input reaches it. Only peers that negotiated the feature send or receive it.
/// The command's optional u32 body is the severity (empty means `ATTENTION_DONE`, as older views send);
/// a severity only ever rises until input answers the tab, and older peers read any nonzero value as alerted.
pub const WIRE_ATTENTION: u32 = 37;
/// Severity by bells rung in one burst: finished, needs the user, failed.
pub const ATTENTION_DONE: u32 = 1;
pub const ATTENTION_HELP: u32 = 2;
pub const ATTENTION_FAILED: u32 = 3;
pub const WIRE_FEATURE_ATTENTION: u32 = 1 << 7;
