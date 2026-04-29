// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure state machine for the rescue TUI — no rendering, no I/O.
//!
//! Split from rendering so unit tests can cover every transition without a
//! TTY or a `TestBackend`.

use iso_probe::{DiscoveredIso, HashVerification, Quirk, SignatureVerification};
use kexec_loader::KexecError;

use crate::theme::Theme;

/// Top-level UI state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// Browsing the discovered ISO list.
    List {
        /// Index of the highlighted row.
        selected: usize,
    },
    /// Confirming a kexec for the selected ISO.
    Confirm {
        /// Index into [`AppState::isos`] of the ISO under confirmation.
        selected: usize,
    },
    /// Editing the kernel command line before kexec.
    EditCmdline {
        /// Index into [`AppState::isos`] of the ISO under edit.
        selected: usize,
        /// Current buffer (starts with the ISO-declared cmdline).
        buffer: String,
        /// Byte-offset cursor position within the buffer.
        cursor: usize,
    },
    /// Showing a fatal-or-classified error after attempted kexec. The
    /// `return_to` index lets the operator return to the failed ISO row
    /// rather than the top of the list. (#85)
    Error {
        /// User-facing diagnostic copied from [`error_diagnostic`].
        message: String,
        /// User-facing remedy hint, if any.
        remedy: Option<String>,
        /// Index of the ISO whose kexec attempt produced this error, so
        /// returning to the List preserves the operator's place.
        return_to: usize,
    },
    /// Help overlay (`?` from any non-edit screen). Stores the screen
    /// to return to so dismissal restores prior state. (#85)
    Help {
        /// Boxed screen to restore on dismiss; boxed to keep the
        /// `Help` variant the same size as the others.
        prior: Box<Screen>,
    },
    /// Quit confirmation prompt. (#85 — was instant exit before.)
    ConfirmQuit {
        /// Boxed screen to restore on cancel.
        prior: Box<Screen>,
    },
    /// Typed-confirmation challenge before kexec'ing an ISO whose
    /// trust state is degraded (YELLOW untrusted signer, GRAY no
    /// verification material). Operator must type `boot` exactly to
    /// continue — prevents muscle-memory Enter on a warned state.
    /// SSH first-connect / HSTS / Gatekeeper "type the app name"
    /// pattern. (#93)
    TrustChallenge {
        /// Real ISO index being kexec'd.
        selected: usize,
        /// Current input buffer (characters typed so far).
        buffer: String,
    },
    /// Verify-now action (#89) — a worker thread streams progress
    /// while re-running hash verification against the selected ISO.
    Verifying {
        /// Real ISO index (not visible-view index) being verified.
        selected: usize,
        /// Bytes hashed so far.
        bytes: u64,
        /// Total bytes to hash (0 if unknown).
        total: u64,
        /// Populated when the worker finishes — caller then updates
        /// the ISO's verification fields in place and dismisses the
        /// screen.
        result: Option<HashVerification>,
    },
    /// User asked to quit; main loop should exit cleanly.
    Quitting,
    /// "Cannot boot" toast surfaced when the operator presses Enter
    /// over a List row that refuses kexec — currently fires for
    /// tier-4 (parse-failed) ISOs, where `confirm_selection`'s silent
    /// no-op left operators wondering whether the keypress registered.
    /// Any key dismisses; `return_to` restores the cursor position.
    /// (#546 — UX review T3C.)
    BlockedToast {
        /// Single-line "Cannot boot: <reason>" message shown in the
        /// popup. Pre-sanitized — see `failed_iso_toast_message`.
        message: String,
        /// Cursor position to restore on dismiss so the operator
        /// returns to the same row they tried.
        return_to: usize,
    },
    /// Consent screen — operator must explicitly acknowledge an
    /// elevated-risk boot path before the kexec proceeds. Per-session
    /// (`AppState::session_consent`): once granted, subsequent boots
    /// in the same session skip this screen. (#347 — Phase 3b of #342.)
    Consent {
        /// What the operator is being asked to consent to. Drives the
        /// rendered prose + the post-grant routing.
        kind: ConsentKind,
        /// Which Confirm-screen ISO to return to after consent
        /// (granted → kexec; dismissed → Confirm).
        selected: usize,
    },
    /// Confirm-before-delete prompt for the highlighted ISO. Opened
    /// from the List screen via the `D` keybinding. `y` unlinks the
    /// ISO + its `<iso>.aegis.toml` sidecar from the data partition;
    /// `n`/`Esc` cancels back to List preserving the cursor.
    ConfirmDelete {
        /// View-cursor index (NOT the real index) of the ISO under
        /// confirmation, mirroring [`Screen::List`]'s coordinate
        /// space so the cursor returns to the same row on cancel.
        selected: usize,
    },
    /// Network overlay (#655 Phase 1B). Shows ethernet interfaces,
    /// lets the operator opt in to DHCP per-interface. `n` from any
    /// non-overlay screen opens it; `Esc`/`q` returns to `prior`.
    /// Networking is opt-in by design — Phase 1A bakes the primitives
    /// but never auto-fires DHCP.
    Network {
        /// Interfaces enumerated at overlay-open time.
        interfaces: Vec<crate::network::NetworkIface>,
        /// Index of the highlighted row in `interfaces`.
        selected: usize,
        /// Per-interface op state (Idle / Pending / Success / Failed).
        op: NetworkOp,
        /// Boxed prior screen so dismiss restores it without churn.
        prior: Box<Screen>,
    },
    /// Catalog browse overlay (#655 Phase 2B PR-C). Lists vendor
    /// ISOs the operator can fetch over the network — same content
    /// as host-side `aegis-boot recommend`, grouped by
    /// [`aegis_catalog::Category`]. `f` from List/Confirm opens it;
    /// `Esc`/`q` returns to `prior`. Selecting an entry opens
    /// [`Screen::CatalogConfirm`] with a free-space precheck.
    Catalog {
        /// Snapshot of `aegis_catalog::CATALOG`. Carried by reference
        /// so the screen state stays cheap to clone for tests.
        entries: &'static [aegis_catalog::Entry],
        /// Index into `entries` of the highlighted row. NOT a
        /// view-row index — section headers do not occupy this
        /// coordinate space (see `render::draw_catalog_overlay` for
        /// the entry-index ↔ visible-row translation).
        selected: usize,
        /// Top-of-viewport row, in the entry-index space. Updated by
        /// the renderer's clamp pass when the cursor moves outside
        /// the visible region.
        scroll: usize,
        /// Boxed prior screen so dismiss restores it without churn.
        prior: Box<Screen>,
    },
    /// Confirm-before-fetch screen for a single catalog entry
    /// (#655 Phase 2B PR-C). Shows ISO size, free space on the
    /// data partition, signature pattern, signing vendor, and the
    /// operator's available actions. Enter starts the fetch worker
    /// (which transitions `op` through Connecting → Downloading →
    /// `VerifyingHash` → `VerifyingSig` → Success/Failed). Esc returns
    /// to the Catalog list, but only when `op` is in a terminal
    /// state — pressing Esc mid-fetch is intentionally locked
    /// because [`aegis_fetch::fetch_catalog_entry`] is synchronous
    /// and uncancellable in this PR. Phase 3 of #655 will add
    /// resumable + cancellable fetches.
    CatalogConfirm {
        /// The catalog entry being fetched.
        entry: &'static aegis_catalog::Entry,
        /// Free bytes on the data partition at confirm-open time.
        /// `0` means the statvfs call failed; the renderer falls
        /// back to "free space unknown" and the operator can still
        /// proceed (mid-stream ENOSPC will surface as
        /// [`CatalogOp::Failed`]).
        free_bytes: u64,
        /// Per-fetch lifecycle state. See [`CatalogOp`].
        op: CatalogOp,
        /// Boxed prior screen so dismiss restores it without churn.
        /// Always `Screen::Catalog`.
        prior: Box<Screen>,
    },
}

/// Network-overlay sub-state. `Idle` is the default at open;
/// `Pending` flips on Enter while the worker thread runs udhcpc;
/// `Success`/`Failed` show the outcome and let the operator pick
/// another interface or close the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkOp {
    /// No active operation. Showing the static interface table.
    Idle,
    /// `udhcpc` running on `iface`. Renderer shows a spinner-line.
    Pending {
        /// Name of the interface DHCP is being attempted on.
        iface: String,
        /// Latest progress hint from the worker (empty until first
        /// `NetworkMsg::Progress` arrives).
        last_status: String,
    },
    /// Lease acquired. Renderer shows IPv4 + gateway + DNS.
    Success {
        /// Interface that succeeded.
        iface: String,
        /// Acquired lease parameters.
        lease: crate::network::NetworkLease,
    },
    /// `udhcpc` exited non-zero or the worker failed to spawn it.
    Failed {
        /// Interface that failed.
        iface: String,
        /// Human-readable failure message.
        err: String,
    },
}

/// Catalog-fetch sub-state. `Idle` is the default at confirm-open;
/// the worker thread drives the transitions through Connecting →
/// Downloading → `VerifyingHash` → `VerifyingSig` → Success/Failed
/// as it pumps [`aegis_fetch::FetchEvent`]s into
/// [`AppState::catalog_progress`]. (#655 Phase 2B PR-C step 2.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogOp {
    /// No active operation. Showing the static confirm screen.
    Idle,
    /// TLS handshake in progress; no bytes received yet. Renderer
    /// shows a spinner-line.
    Connecting,
    /// Bytes are streaming. `total` is `None` for chunked /
    /// streaming responses (no Content-Length); UIs fall back to
    /// a spinner + bytes-so-far in that case.
    Downloading {
        /// Bytes downloaded so far.
        bytes: u64,
        /// Content-Length from the server, or `None` if unknown.
        total: Option<u64>,
    },
    /// Streaming SHA-256 of the ISO is being computed.
    VerifyingHash,
    /// PGP signature is being verified against the pinned vendor
    /// cert. Last step before terminal Success.
    VerifyingSig,
    /// Fetch + verification both succeeded. Outcome carries the
    /// verified ISO path + the cert fingerprint that authenticated
    /// it (surfaced in audit logs).
    Success(aegis_fetch::FetchOutcome),
    /// Any error from the fetch path. The string is operator-
    /// readable and is rendered verbatim.
    Failed(String),
}

/// Reason for a [`Screen::Consent`] prompt. Each variant pairs with a
/// specific operator decision the system needs explicit ack for. The
/// shipped policy is *allow boot of any media but maintain the chain
/// of trust as opt-in for high security* — the consent screen is the
/// hinge where the operator opts down from "trusted only" to "any
/// media." (#347 maintainer alignment.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentKind {
    /// The selected ISO carries an installer that can erase disks on
    /// the host machine if the operator picks the wrong target inside
    /// the ISO's own boot menu. Triggered by [`DiscoveredIso`]'s
    /// `contains_installer` flag (#131). The Confirm screen renders an
    /// inline warning today; consent is the second-step gate before
    /// kexec proceeds.
    InstallerCanEraseDisks,
    /// The selected entry is a parse-failed (tier-4) ISO and the
    /// operator wants to attempt boot anyway. Currently a no-op
    /// because iso-parser couldn't extract a kernel/initrd, but
    /// recording the consent makes the eventual kernel-extraction
    /// failure path's diagnostic clearer ("force-boot consented but
    /// no kernel found in this ISO").
    Tier4ForceBoot,
}

impl ConsentKind {
    /// Short imperative title for the consent-screen header.
    #[must_use]
    pub fn title(&self) -> &'static str {
        match self {
            Self::InstallerCanEraseDisks => "Confirm: installer can erase disks",
            Self::Tier4ForceBoot => "Confirm: force boot of unparsable ISO",
        }
    }

    /// Multi-line operator-facing prose describing what consent means
    /// for this kind. Returned as a `Vec<&'static str>` so the
    /// renderer can wrap to terminal width without re-parsing.
    #[must_use]
    pub fn prose(&self) -> &'static [&'static str] {
        match self {
            Self::InstallerCanEraseDisks => &[
                "This ISO contains an OS installer.",
                "If the ISO's own boot menu defaults to 'Install',",
                "DISKS ON THIS MACHINE MAY BE ERASED — including",
                "the aegis-boot stick itself, if you pick it as a target.",
                "",
                "Press 'y' to grant consent for the rest of this rescue session.",
                "Press Esc to return to the Confirm screen and pick differently.",
            ],
            Self::Tier4ForceBoot => &[
                "This ISO failed to parse — iso-parser could not extract",
                "a kernel + initrd from its layout.",
                "",
                "Force-boot will attempt the kexec anyway, but is expected",
                "to fail with a clear error explaining what's missing.",
                "Useful for diagnosing parser-vs-distro disagreement.",
                "",
                "Press 'y' to grant force-boot consent for the rest of this session.",
                "Press Esc to return to the list.",
            ],
        }
    }
}

/// Top-level application state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    /// All discovered ISOs.
    pub isos: Vec<DiscoveredIso>,
    /// `.iso` files on disk that iso-parser couldn't extract boot
    /// entries from (mount failure, unfamiliar layout, truncated
    /// file). Carried alongside [`Self::isos`] so the TUI can render
    /// tier-4 rows with the per-file reason — the list stays
    /// exhaustive even when parse fails. Populated from
    /// [`iso_probe::DiscoveryReport::failed`] in `main.rs`. (#457)
    pub failed_isos: Vec<iso_probe::FailedIso>,
    /// Current screen.
    pub screen: Screen,
    /// Per-ISO cmdline overrides keyed by index. When absent the
    /// ISO-declared default from `iso-probe` is used.
    pub cmdline_overrides: std::collections::HashMap<usize, String>,
    /// Active color theme (resolved from `AEGIS_THEME` env var).
    pub theme: Theme,
    /// Secure Boot enforcement status, detected once at startup.
    /// Rendered in the persistent header banner. (#85)
    pub secure_boot: SecureBootStatus,
    /// TPM availability, detected once at startup. Rendered in the
    /// persistent header banner. (#85)
    pub tpm: TpmStatus,
    /// Set when the operator picked the rescue-shell entry (#90).
    /// Causes `main.rs::run()` to exit with [`RESCUE_SHELL_EXIT_CODE`]
    /// so the initramfs `/init` script can drop to busybox.
    pub shell_requested: bool,
    /// Active substring filter on the List screen (#85 Tier 2).
    /// Empty means show all ISOs. Matches against label + path,
    /// case-insensitive.
    pub filter: String,
    /// True while the user is actively typing into the filter input
    /// (`/` opened, Enter / Esc closes).
    pub filter_editing: bool,
    /// Sort order for the List screen view. Cycled with `s`.
    pub sort_order: SortOrder,
    /// Paths that `iso_probe::discover()` was asked to scan. Surfaced
    /// in the empty-state screen so operators see where we looked
    /// (rather than "no bootable ISOs found" with no actionable
    /// detail). Populated by `main.rs` after reading `AEGIS_ISO_ROOTS`;
    /// default empty in tests. (#85 Tier 2)
    pub scanned_roots: Vec<std::path::PathBuf>,
    /// Count of `.iso` files found on disk under `scanned_roots` that
    /// DID NOT parse into a `DiscoveredIso` (malformed layout, mount
    /// failure, etc). Sourced from
    /// [`iso_probe::DiscoveryReport::failed`]'s length plus any
    /// additional files the on-disk walk saw that iso-parser never
    /// attempted. Surfaced as a yellow inline band above the List
    /// when >0 so the operator knows the list is incomplete.
    /// (#85 Tier 2 last child.) #457 will replace this banner with
    /// per-ISO tier-4 rows sourced from `DiscoveryReport::failed`.
    pub skipped_iso_count: usize,
    /// Which pane holds keyboard focus on the List screen. Defaults to
    /// [`Pane::List`]; toggled with `Tab`. (#458)
    pub pane: Pane,
    /// Vertical scroll offset for the info pane (tier-specific detail
    /// view). Increments when `↑`/`↓` are pressed while `pane` is
    /// [`Pane::Info`]. Resets to 0 when the list selection changes —
    /// per-ISO scrollback would be confusing. (#458 / #459)
    pub info_scroll: u16,
    /// Non-blocking warning banner rendered on the Confirm screen when
    /// the verify-now JSONL audit log (#548) failed to write. Lets the
    /// operator see "verdict shown but not persisted" inline with the
    /// verdict instead of having that signal go only to journald where
    /// the booted-stick operator can't see it. Set by `main.rs` when
    /// `save_verify_audit_log()` returns `Err`; cleared whenever a new
    /// verify succeeds, since the new line replaces the missing one.
    /// (#602)
    pub audit_warning: Option<String>,
    /// Per-session consent flag (#347). When true, the operator has
    /// explicitly acknowledged at least one elevated-risk boot path
    /// (installer-can-erase, tier-4 force-boot) and subsequent boots
    /// in the same session skip the [`Screen::Consent`] gate. Resets
    /// to false on every rescue-tui startup — boot-decisions made by
    /// last week's operator do not carry forward to today's. The flag
    /// is inspected at the Confirm-screen Enter handler before
    /// dispatching to [`Self::is_kexec_blocked`] / kexec.
    pub session_consent: bool,
    /// Most recent successful DHCP lease, if any. Set by
    /// [`Self::network_finish_dhcp`] on the Ok branch and consumed
    /// by `Screen::Catalog` to gate fetch availability + by the
    /// header-banner renderer to display the active IP/gateway
    /// (header rendering lands in #655 PR-C step 3).
    pub network_lease: Option<crate::network::NetworkLease>,
}

/// Sort order applied to the List view. Cycled with the `s` key.
/// Persisted only in-process — not saved across reboots (operators may
/// want different defaults per use). (#85 Tier 2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Alphabetical by label (default).
    Name,
    /// Largest ISOs first — usually the "main" install media.
    SizeDesc,
    /// Group by distribution family.
    Distro,
}

/// An entry displayed on the List screen. Includes discovered ISOs
/// (by real index), parse-failed ISOs (tier-4), and synthetic entries
/// like the always-present rescue shell (#90, #459).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewEntry {
    /// Real ISO at this index into [`AppState::isos`].
    Iso(usize),
    /// Parse-failed `.iso` at this index into [`AppState::failed_isos`].
    /// Rendered as a tier-4 row with a descriptive reason in the info
    /// pane; boot disabled.
    FailedIso(usize),
    /// Synthetic "drop to rescue shell" entry (#90). Selecting it
    /// exits rescue-tui with [`RESCUE_SHELL_EXIT_CODE`] so the
    /// initramfs `/init` can recognize the operator request and
    /// `exec /bin/sh`.
    RescueShell,
}

/// Exit code rescue-tui returns when the operator picks the synthetic
/// "rescue shell" entry. `/init` switches on this value and drops to
/// busybox instead of treating the exit as an error. (#90)
pub const RESCUE_SHELL_EXIT_CODE: i32 = 42;

impl SortOrder {
    /// Cycle to the next sort order.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::SizeDesc,
            Self::SizeDesc => Self::Distro,
            Self::Distro => Self::Name,
        }
    }

    /// Short label for the header / footer.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::SizeDesc => "size↓",
            Self::Distro => "distro",
        }
    }
}

/// Which pane holds keyboard focus on the List screen. List selection
/// moves with ↑↓ when focus is `List`; the info-pane scroll offset
/// moves with ↑↓ when focus is `Info`. `Tab` toggles. (#458)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    /// Left pane — ISO list. Default focus at startup.
    #[default]
    List,
    /// Right pane — info/detail view for the selected ISO.
    Info,
}

impl Pane {
    /// Swap between `List` and `Info`. Used by the `Tab` key handler.
    #[must_use]
    pub fn toggle(self) -> Self {
        match self {
            Self::List => Self::Info,
            Self::Info => Self::List,
        }
    }
}

/// Coarse Secure Boot enforcement state, detected once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureBootStatus {
    /// EFI variable says SB is enforcing.
    Enforcing,
    /// EFI variable present but SB disabled / setup mode.
    Disabled,
    /// Couldn't read EFI vars (not booted via UEFI, or efivars not mounted).
    Unknown,
}

/// TPM availability for pre-kexec PCR measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpmStatus {
    /// `/dev/tpm0` or `/dev/tpmrm0` present.
    Available,
    /// No TPM device exposed.
    Absent,
}

impl SecureBootStatus {
    /// Detect SB status from /sys/firmware/efi/efivars. Best-effort —
    /// returns Unknown on any read error so the TUI can boot anywhere.
    ///
    /// Checks, in order:
    ///   1. Global-variables-namespace GUID suffix
    ///      (`SecureBoot-8be4df61-…`) — the upstream spec name.
    ///   2. Plain `SecureBoot` — seen on some older kernels / distros
    ///      that rename on mount.
    ///   3. Directory scan for anything starting with `SecureBoot-` —
    ///      catches OVMF firmware builds that use a different-looking
    ///      suffix (observed under our QEMU `SecBoot` shakedown, #118).
    ///
    /// EFI var wire format for all three cases is identical: 4 bytes
    /// of attributes, then one byte of data (0/1). We read `bytes[4]`.
    #[must_use]
    pub fn detect() -> Self {
        // EFI var format: 4 bytes attributes, 1 byte data (0/1).
        let candidates = [
            "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c",
            "/sys/firmware/efi/efivars/SecureBoot",
        ];
        for path in candidates {
            if let Some(s) = Self::read_sb_bit(std::path::Path::new(path)) {
                return s;
            }
        }
        // Fallback: scan the efivars directory for any variable whose
        // filename starts with "SecureBoot-". Covers OVMF firmware
        // builds that publish the variable under a different suffix
        // than the upstream-spec one we check above. (#118)
        if let Ok(entries) = std::fs::read_dir("/sys/firmware/efi/efivars") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_s = name.to_string_lossy();
                if name_s.starts_with("SecureBoot-")
                    && let Some(s) = Self::read_sb_bit(&entry.path())
                {
                    return s;
                }
            }
        }
        Self::Unknown
    }

    /// Read and interpret the 5-byte `[attrs][value]` layout at `path`.
    /// Returns `None` if the file can't be read or is truncated.
    /// Extracted so `detect`'s scan-fallback can reuse the same parse
    /// without duplicating the magic-offset read.
    fn read_sb_bit(path: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let value = *bytes.get(4)?;
        Some(if value == 1 {
            Self::Enforcing
        } else {
            Self::Disabled
        })
    }

    /// Short user-facing label for the TUI header banner.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Enforcing => "SB:enforcing",
            Self::Disabled => "SB:disabled",
            Self::Unknown => "SB:unknown",
        }
    }
}

impl TpmStatus {
    /// Detect TPM presence by checking for `/dev/tpm*` device nodes.
    #[must_use]
    pub fn detect() -> Self {
        if std::path::Path::new("/dev/tpm0").exists()
            || std::path::Path::new("/dev/tpmrm0").exists()
        {
            Self::Available
        } else {
            Self::Absent
        }
    }

    /// Short user-facing label for the TUI header banner.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Available => "TPM:available",
            Self::Absent => "TPM:none",
        }
    }
}

impl AppState {
    /// Build a fresh state from a discovery result.
    #[must_use]
    pub fn new(isos: Vec<DiscoveredIso>) -> Self {
        Self {
            isos,
            failed_isos: Vec::new(),
            screen: Screen::List { selected: 0 },
            cmdline_overrides: std::collections::HashMap::new(),
            theme: Theme::default_theme(),
            secure_boot: SecureBootStatus::detect(),
            tpm: TpmStatus::detect(),
            shell_requested: false,
            filter: String::new(),
            filter_editing: false,
            sort_order: SortOrder::Name,
            scanned_roots: Vec::new(),
            skipped_iso_count: 0,
            pane: Pane::default(),
            info_scroll: 0,
            audit_warning: None,
            session_consent: false,
            network_lease: None,
        }
    }

    /// Open the [`Screen::Consent`] screen for the given consent kind
    /// against the given Confirm-screen ISO. Called from the Confirm
    /// Enter handler when consent is required AND not yet granted in
    /// this session. (#347)
    pub fn enter_consent(&mut self, kind: ConsentKind, selected: usize) {
        self.screen = Screen::Consent { kind, selected };
    }

    /// Grant consent for the current session. Returns the `selected`
    /// index from the consent screen so the caller can chain into the
    /// Confirm-screen path the operator was originally trying to
    /// reach. Resets the screen to Confirm; subsequent kexec attempts
    /// in the same session no longer hit the consent screen. (#347)
    pub fn grant_consent(&mut self) -> Option<usize> {
        if let Screen::Consent { selected, .. } = self.screen {
            self.session_consent = true;
            self.screen = Screen::Confirm { selected };
            Some(selected)
        } else {
            None
        }
    }

    /// Cancel a pending consent prompt without granting. Returns the
    /// operator to the Confirm screen for the same ISO without
    /// persisting consent. (#347)
    pub fn cancel_consent(&mut self) {
        if let Screen::Consent { selected, .. } = self.screen {
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Whether the Confirm-screen Enter path needs to detour through
    /// the consent screen for the given ISO. Returns `Some(kind)` if
    /// a consent prompt is required for some reason, `None` if the
    /// path can proceed straight to the existing kexec / trust
    /// challenge flow. Caller is `main.rs`'s Confirm-Enter handler.
    /// (#347)
    #[must_use]
    pub fn consent_required_for(&self, iso_idx: usize) -> Option<ConsentKind> {
        // Per-session: if consent already granted, no further prompts.
        if self.session_consent {
            return None;
        }
        let iso = self.isos.get(iso_idx)?;
        if iso.contains_installer {
            return Some(ConsentKind::InstallerCanEraseDisks);
        }
        None
    }

    /// Set the non-blocking audit-log warning banner shown on the Confirm
    /// screen. Called by `main.rs` when `save_verify_audit_log()` fails
    /// after a successful verify-now. The verdict still propagates and
    /// kexec-gating is unaffected — only the audit-trail integrity
    /// signal is surfaced. (#602)
    pub fn set_audit_warning(&mut self, msg: impl Into<String>) {
        self.audit_warning = Some(msg.into());
    }

    /// Clear the audit-log warning banner. Called when a subsequent
    /// verify-now succeeds and writes a fresh JSONL line — the new line
    /// supersedes the previous "missing" state, so the warning would be
    /// stale. (#602)
    pub fn clear_audit_warning(&mut self) {
        self.audit_warning = None;
    }

    /// Attach the list of ISOs that iso-parser couldn't extract boot
    /// entries from — carried alongside the successful ISOs so the
    /// TUI can render tier-4 rows in the list instead of hiding them
    /// behind a count. Populated from
    /// [`iso_probe::DiscoveryReport::failed`] in `main.rs`. Tests
    /// default to an empty vec via [`Self::new`]. (#457)
    #[must_use]
    pub fn with_failed_isos(mut self, failed: Vec<iso_probe::FailedIso>) -> Self {
        self.failed_isos = failed;
        self
    }

    /// Attach the paths `discover()` was asked to scan. Called from
    /// `main.rs` before entering the event loop so the empty-state
    /// screen can tell the operator where we looked. Safe to skip in
    /// tests — field defaults to empty and renders a no-paths variant.
    /// (#85 Tier 2)
    #[must_use]
    pub fn with_scanned_roots(mut self, roots: Vec<std::path::PathBuf>) -> Self {
        self.scanned_roots = roots;
        self
    }

    /// Record how many `.iso` files on disk were silently skipped by
    /// `discover()` (malformed ISO, unsupported layout, parse failure).
    /// Populated by `main.rs` — tests default to 0 via `AppState::new()`.
    /// (#85 Tier 2 last child — inline error band)
    #[must_use]
    pub fn with_skipped_iso_count(mut self, n: usize) -> Self {
        self.skipped_iso_count = n;
        self
    }

    /// Ordered list of entries displayed on the List screen.
    ///
    /// Layout (#459):
    /// 1. Successful ISOs (tier 1/2/3/5/6) in the active sort order,
    ///    filtered by the active substring filter.
    /// 2. Parse-failed ISOs (tier 4) in alphabetical order — always
    ///    surfaced so the operator sees every file on the stick. Also
    ///    subject to the substring filter (matches against `iso_path`).
    /// 3. The synthetic [`ViewEntry::RescueShell`] — always present so
    ///    operators have an escape hatch to a signed busybox shell.
    #[must_use]
    pub fn visible_entries(&self) -> Vec<ViewEntry> {
        let mut entries: Vec<ViewEntry> = self
            .visible_indices()
            .into_iter()
            .map(ViewEntry::Iso)
            .collect();
        let needle = self.filter.to_ascii_lowercase();
        let mut failed: Vec<(usize, String)> = self
            .failed_isos
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                if needle.is_empty() {
                    return true;
                }
                f.iso_path
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&needle)
            })
            .map(|(i, f)| {
                (
                    i,
                    f.iso_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            })
            .collect();
        failed.sort_by(|a, b| a.1.cmp(&b.1));
        entries.extend(failed.into_iter().map(|(i, _)| ViewEntry::FailedIso(i)));
        entries.push(ViewEntry::RescueShell);
        entries
    }

    /// Translate a List-cursor position to the selected [`ViewEntry`].
    #[must_use]
    pub fn view_entry(&self, cursor: usize) -> Option<ViewEntry> {
        self.visible_entries().into_iter().nth(cursor)
    }

    /// Indices into [`Self::isos`] in display order — applies both the
    /// substring filter and the active sort. This is the ISO-only view
    /// (no rescue-shell entry). Use [`Self::visible_entries`] when
    /// rendering or navigating. (#85 Tier 2)
    #[must_use]
    pub fn visible_indices(&self) -> Vec<usize> {
        let needle = self.filter.to_ascii_lowercase();
        let mut indices: Vec<usize> = self
            .isos
            .iter()
            .enumerate()
            .filter(|(_, iso)| {
                if needle.is_empty() {
                    return true;
                }
                let label = iso.label.to_ascii_lowercase();
                let path = iso.iso_path.to_string_lossy().to_ascii_lowercase();
                label.contains(&needle) || path.contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();
        match self.sort_order {
            SortOrder::Name => {
                indices.sort_by(|&a, &b| self.isos[a].label.cmp(&self.isos[b].label));
            }
            SortOrder::SizeDesc => {
                indices.sort_by(|&a, &b| {
                    self.isos[b]
                        .size_bytes
                        .unwrap_or(0)
                        .cmp(&self.isos[a].size_bytes.unwrap_or(0))
                });
            }
            SortOrder::Distro => {
                indices.sort_by(|&a, &b| {
                    let da = format!("{:?}", self.isos[a].distribution);
                    let db = format!("{:?}", self.isos[b].distribution);
                    da.cmp(&db)
                        .then_with(|| self.isos[a].label.cmp(&self.isos[b].label))
                });
            }
        }
        indices
    }

    /// Translate a List-cursor position to a real index into `isos`,
    /// or None if the view is empty / cursor out of range.
    #[must_use]
    pub fn real_index(&self, cursor: usize) -> Option<usize> {
        self.visible_indices().get(cursor).copied()
    }

    /// Cycle to the next sort order. (#85 Tier 2)
    pub fn cycle_sort(&mut self) {
        self.sort_order = self.sort_order.next();
        // Reset cursor so the view doesn't point past the (possibly
        // reordered) list end.
        if let Screen::List { selected } = &mut self.screen {
            *selected = 0;
        }
    }

    /// Open the filter editor. (#85 Tier 2)
    pub fn open_filter(&mut self) {
        if matches!(self.screen, Screen::List { .. }) {
            self.filter_editing = true;
        }
    }

    /// Append a character to the filter while editing.
    pub fn filter_push(&mut self, c: char) {
        if self.filter_editing {
            self.filter.push(c);
            if let Screen::List { selected } = &mut self.screen {
                *selected = 0;
            }
        }
    }

    /// Delete the last character from the filter while editing.
    pub fn filter_backspace(&mut self) {
        if self.filter_editing {
            self.filter.pop();
            if let Screen::List { selected } = &mut self.screen {
                *selected = 0;
            }
        }
    }

    /// Commit the filter (Enter): keep the query, exit editing mode.
    pub fn filter_commit(&mut self) {
        self.filter_editing = false;
    }

    /// Cancel the filter (Esc): clear the query and exit editing mode.
    pub fn filter_cancel(&mut self) {
        self.filter_editing = false;
        self.filter.clear();
        if let Screen::List { selected } = &mut self.screen {
            *selected = 0;
        }
    }

    /// Effective cmdline for the ISO at `idx` — the user's override if one
    /// exists, otherwise the ISO-declared default (or empty string).
    #[must_use]
    pub fn effective_cmdline(&self, idx: usize) -> String {
        self.cmdline_overrides
            .get(&idx)
            .cloned()
            .or_else(|| self.isos.get(idx)?.cmdline.clone())
            .unwrap_or_default()
    }

    /// Whether the ISO at `idx` carries a quirk that blocks kexec entirely
    /// (e.g. Windows installers — wrong boot protocol), OR the ISO failed
    /// integrity verification (hash mismatch / forged signature). The TUI
    /// uses this to disable the Enter binding on the Confirm screen.
    ///
    /// Hash and signature failures are enforced as hard blocks — the red
    /// `✗ MISMATCH` / `✗ FORGED` indicator on the Confirm screen would
    /// otherwise be advisory-only, which lets a physical-access attacker
    /// boot a tampered ISO by clicking through the warning. (#55)
    #[must_use]
    pub fn is_kexec_blocked(&self, idx: usize) -> bool {
        let Some(iso) = self.isos.get(idx) else {
            return false;
        };
        if iso.quirks.contains(&Quirk::NotKexecBootable) {
            return true;
        }
        if matches!(iso.hash_verification, HashVerification::Mismatch { .. }) {
            return true;
        }
        if matches!(
            iso.signature_verification,
            SignatureVerification::Forged { .. }
        ) {
            return true;
        }
        false
    }

    /// Transition confirm → edit-cmdline, seeding the buffer with whatever
    /// is currently effective for the selected ISO.
    pub fn enter_cmdline_editor(&mut self) {
        let Screen::Confirm { selected } = self.screen else {
            return;
        };
        let buffer = self.effective_cmdline(selected);
        let cursor = buffer.len();
        self.screen = Screen::EditCmdline {
            selected,
            buffer,
            cursor,
        };
    }

    /// Commit the editor's buffer as the override and return to Confirm.
    pub fn commit_cmdline_edit(&mut self) {
        let Screen::EditCmdline {
            selected, buffer, ..
        } = std::mem::replace(&mut self.screen, Screen::Quitting)
        else {
            return;
        };
        self.cmdline_overrides.insert(selected, buffer);
        self.screen = Screen::Confirm { selected };
    }

    /// Cancel the editor and return to Confirm without saving.
    pub fn cancel_cmdline_edit(&mut self) {
        if let Screen::EditCmdline { selected, .. } = self.screen {
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Insert a single character at the cursor.
    pub fn cmdline_insert(&mut self, ch: char) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        buffer.insert(*cursor, ch);
        *cursor += ch.len_utf8();
    }

    /// Delete one char before the cursor (backspace).
    pub fn cmdline_backspace(&mut self) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        if *cursor == 0 {
            return;
        }
        let new_cursor = prev_char_boundary(buffer, *cursor);
        buffer.replace_range(new_cursor..*cursor, "");
        *cursor = new_cursor;
    }

    /// Move cursor left one char (saturating).
    pub fn cmdline_cursor_left(&mut self) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        if *cursor == 0 {
            return;
        }
        *cursor = prev_char_boundary(buffer, *cursor);
    }

    /// Move cursor right one char (saturating).
    pub fn cmdline_cursor_right(&mut self) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        if *cursor >= buffer.len() {
            return;
        }
        let new_cursor = (*cursor + 1..=buffer.len())
            .find(|candidate| buffer.is_char_boundary(*candidate))
            .unwrap_or(buffer.len());
        *cursor = new_cursor;
    }

    /// Jump cursor to the start of the line (Home / Ctrl+A). #544.
    pub fn cmdline_cursor_home(&mut self) {
        let Screen::EditCmdline { cursor, .. } = &mut self.screen else {
            return;
        };
        *cursor = 0;
    }

    /// Jump cursor to the end of the line (End / Ctrl+E). #544.
    pub fn cmdline_cursor_end(&mut self) {
        let Screen::EditCmdline { buffer, cursor, .. } = &mut self.screen else {
            return;
        };
        *cursor = buffer.len();
    }

    /// Jump to the first visible row (vim `g`). (#85). The visible
    /// view always has at least one row (the rescue-shell entry
    /// since #90), so the `has_view` check is trivially true — kept
    /// for defence in depth in case that invariant ever changes.
    pub fn move_to_first(&mut self) {
        let has_view = !self.visible_entries().is_empty();
        if let Screen::List { selected } = &mut self.screen
            && has_view
        {
            *selected = 0;
        }
    }

    /// Jump to the last visible row (vim `G`). With #90 the last row
    /// is always the rescue-shell entry.
    pub fn move_to_last(&mut self) {
        let view_len = self.visible_entries().len();
        if let Screen::List { selected } = &mut self.screen
            && view_len > 0
        {
            *selected = view_len - 1;
        }
    }

    /// Advance the visible cursor up (negative) or down (positive),
    /// saturating at the visible-list bounds. Cursor indexes the full
    /// entries list (ISOs + rescue shell), not just ISOs (#85, #90).
    ///
    /// Also resets `info_scroll` to 0 whenever the cursor actually
    /// moves — per-ISO scrollback in the info pane would be
    /// confusing (#458).
    pub fn move_selection(&mut self, delta: i32) {
        let view_len = self.visible_entries().len();
        let Screen::List { selected } = &mut self.screen else {
            return;
        };
        if view_len == 0 {
            return;
        }
        let max = view_len - 1;
        let before = *selected;
        if delta < 0 {
            let step = delta.unsigned_abs() as usize;
            *selected = selected.saturating_sub(step);
        } else {
            let step = usize::try_from(delta).unwrap_or(0);
            *selected = selected.saturating_add(step).min(max);
        }
        if *selected != before {
            self.info_scroll = 0;
        }
    }

    /// Swap the keyboard-focus pane (List ↔ Info). No-op outside the
    /// List screen. (#458)
    pub fn toggle_pane(&mut self) {
        if matches!(self.screen, Screen::List { .. }) {
            self.pane = self.pane.toggle();
        }
    }

    /// Scroll the info pane up (negative) or down (positive),
    /// saturating at zero. Upper bound is enforced at render time
    /// (the pane knows its own height + content length). (#458)
    pub fn move_info_scroll(&mut self, delta: i32) {
        if delta < 0 {
            let step = u16::try_from(delta.unsigned_abs()).unwrap_or(u16::MAX);
            self.info_scroll = self.info_scroll.saturating_sub(step);
        } else {
            let step = u16::try_from(delta).unwrap_or(u16::MAX);
            self.info_scroll = self.info_scroll.saturating_add(step);
        }
    }

    /// Transition list → confirm for the highlighted ISO. The cursor
    /// on List indexes the full entries view (ISOs + rescue shell);
    /// ISO selections route to Confirm, shell selections are handled
    /// by [`Self::is_shell_selected`] (main.rs exits with
    /// [`RESCUE_SHELL_EXIT_CODE`] in that case).
    pub fn confirm_selection(&mut self) {
        if let Screen::List { selected } = self.screen {
            match self.view_entry(selected) {
                Some(ViewEntry::Iso(real)) => {
                    self.screen = Screen::Confirm { selected: real };
                }
                Some(ViewEntry::FailedIso(idx)) => {
                    // #546: tier-4 rows refused Enter silently. Surface
                    // the parse-failed reason as a toast rather than
                    // leaving the operator wondering if Enter registered.
                    let message = self.failed_iso_toast_message(idx);
                    self.screen = Screen::BlockedToast {
                        message,
                        return_to: selected,
                    };
                }
                _ => {}
            }
        }
    }

    /// Compose the "Cannot boot:" toast message for a tier-4 (`FailedIso`)
    /// row at `idx`. Falls back to a generic message if the index is
    /// out of bounds (defensive — `view_entry` should have validated it).
    fn failed_iso_toast_message(&self, idx: usize) -> String {
        match self.failed_isos.get(idx) {
            Some(failed) => format!("Cannot boot: parse failed — {}", failed.reason),
            None => "Cannot boot: parse failed (no reason available)".to_string(),
        }
    }

    /// Dismiss the `BlockedToast` and return the cursor to the row the
    /// operator originally tried. Any-key dismissal — `main.rs` binds
    /// every keypress to this in the `BlockedToast` branch.
    pub fn dismiss_blocked_toast(&mut self) {
        if let Screen::BlockedToast { return_to, .. } = self.screen {
            self.screen = Screen::List {
                selected: return_to,
            };
        }
    }

    /// Whether the List cursor currently highlights the synthetic
    /// rescue-shell entry. main.rs uses this to branch on Enter. (#90)
    #[must_use]
    pub fn is_shell_selected(&self) -> bool {
        if let Screen::List { selected } = self.screen {
            matches!(self.view_entry(selected), Some(ViewEntry::RescueShell))
        } else {
            false
        }
    }

    /// Transition confirm → list (cancel). Maps the real ISO index
    /// back to its cursor position in the current entries view (ISO
    /// entries + rescue shell, #90). Falls back to 0 if the ISO is
    /// no longer visible (e.g. filter changed).
    pub fn cancel_confirmation(&mut self) {
        if let Screen::Confirm { selected } = self.screen {
            let cursor = self
                .visible_entries()
                .iter()
                .position(|e| matches!(e, ViewEntry::Iso(i) if *i == selected))
                .unwrap_or(0);
            self.screen = Screen::List { selected: cursor };
        }
    }

    /// Record a kexec failure and transition to the Error screen.
    pub fn record_kexec_error(&mut self, err: &KexecError) {
        // If the error occurred during a Confirm flow, enrich with
        // ISO-specific context (sibling key discovery for
        // SignatureRejected).
        let (iso, return_to) = match &self.screen {
            Screen::Confirm { selected } | Screen::EditCmdline { selected, .. } => {
                (self.isos.get(*selected), *selected)
            }
            _ => (None, 0),
        };
        let (message, remedy) = error_diagnostic_with_iso(err, iso);
        self.screen = Screen::Error {
            message,
            remedy,
            return_to,
        };
    }

    /// Open the [`Screen::ConfirmDelete`] prompt against the highlighted
    /// list cursor. Returns the path of the ISO that's about to be
    /// confirmed (caller may show a preview line). Returns `None` and
    /// leaves screen unchanged when the cursor doesn't point at a real
    /// ISO row (`FailedIso` / `RescueShell` are non-deletable here —
    /// those rows aren't backed by a removable on-disk ISO file under
    /// our management).
    pub fn enter_delete(&mut self, selected: usize) -> Option<std::path::PathBuf> {
        if !matches!(self.screen, Screen::List { .. }) {
            return None;
        }
        let real_idx = self.real_index(selected)?;
        let path = self.isos.get(real_idx)?.iso_path.clone();
        self.screen = Screen::ConfirmDelete { selected };
        Some(path)
    }

    /// Cancel the delete prompt, returning to the List with the cursor
    /// preserved at the same row.
    pub fn cancel_delete(&mut self) {
        if let Screen::ConfirmDelete { selected } = self.screen {
            self.screen = Screen::List { selected };
        }
    }

    /// Apply a successful unlink: drop the corresponding ISO from
    /// [`Self::isos`], clamp the cursor, and return to the List screen.
    /// Caller is responsible for performing the actual filesystem
    /// removal BEFORE invoking this; on filesystem failure use
    /// [`Self::record_delete_error`] instead.
    pub fn delete_completed(&mut self) {
        let Screen::ConfirmDelete { selected } = self.screen else {
            return;
        };
        let Some(real_idx) = self.real_index(selected) else {
            self.screen = Screen::List { selected: 0 };
            return;
        };
        // Drop any cmdline override pinned to this index, AND shift
        // every override at a higher index down by one — vec-remove
        // shifts the trailing entries' positions. Without this fix
        // an operator who'd edited the cmdline of an ISO past the
        // deleted one would see their edit attached to the wrong row.
        self.cmdline_overrides.remove(&real_idx);
        let shifted: std::collections::HashMap<usize, String> = self
            .cmdline_overrides
            .drain()
            .map(|(idx, val)| {
                if idx > real_idx {
                    (idx - 1, val)
                } else {
                    (idx, val)
                }
            })
            .collect();
        self.cmdline_overrides = shifted;
        self.isos.remove(real_idx);
        let new_total = self.visible_entries().len();
        let clamped = if new_total == 0 {
            0
        } else {
            selected.min(new_total - 1)
        };
        self.screen = Screen::List { selected: clamped };
    }

    /// Record a delete-time filesystem error and surface it on the
    /// Error screen. Mirrors [`Self::record_kexec_error`] so the
    /// post-failure UX is uniform — one Error screen variant the
    /// operator already knows how to dismiss.
    pub fn record_delete_error(&mut self, err: &str) {
        let return_to = match &self.screen {
            Screen::ConfirmDelete { selected } => *selected,
            _ => 0,
        };
        self.screen = Screen::Error {
            message: format!("Delete failed: {err}"),
            remedy: Some(
                "Check that the data partition is mounted read-write \
                 and that no other process holds the ISO open."
                    .to_string(),
            ),
            return_to,
        };
    }

    /// Open the [`Screen::Network`] overlay. Caller passes the
    /// already-enumerated iface list (state machine is pure — no I/O
    /// here). Stores the current screen as `prior` so `cancel_network`
    /// can restore it on dismiss.
    pub fn enter_network(&mut self, interfaces: Vec<crate::network::NetworkIface>) {
        // Don't stack — re-opening from inside Network is a no-op.
        if matches!(self.screen, Screen::Network { .. }) {
            return;
        }
        let prior = std::mem::replace(&mut self.screen, Screen::Quitting);
        self.screen = Screen::Network {
            interfaces,
            selected: 0,
            op: NetworkOp::Idle,
            prior: Box::new(prior),
        };
    }

    /// Cancel the Network overlay, returning to the screen that was
    /// active when it was opened.
    pub fn cancel_network(&mut self) {
        if let Screen::Network { prior, .. } = std::mem::replace(&mut self.screen, Screen::Quitting)
        {
            self.screen = *prior;
        }
    }

    /// Move the Network-overlay cursor by `delta` (`-1` = up, `+1` =
    /// down). Clamps to `[0, interfaces.len() - 1]`.
    pub fn network_move_selection(&mut self, delta: isize) {
        if let Screen::Network {
            interfaces,
            selected,
            ..
        } = &mut self.screen
        {
            let len = interfaces.len();
            if len == 0 {
                *selected = 0;
                return;
            }
            let max = len.saturating_sub(1);
            let new = i64::try_from(*selected).unwrap_or(0)
                + i64::from(i32::try_from(delta).unwrap_or(0));
            let clamped = new.clamp(0, i64::try_from(max).unwrap_or(0));
            *selected = usize::try_from(clamped).unwrap_or(0);
        }
    }

    /// Refresh the iface table (e.g. operator pressed `r`). Replaces
    /// the cached list with `next` and resets the cursor + op state.
    pub fn network_refresh(&mut self, next: Vec<crate::network::NetworkIface>) {
        if let Screen::Network {
            interfaces,
            selected,
            op,
            ..
        } = &mut self.screen
        {
            *interfaces = next;
            *selected = 0;
            *op = NetworkOp::Idle;
        }
    }

    /// Mark `iface` as DHCP-pending. Returns the iface name iff the
    /// transition fired (caller spawns the worker). Returns `None`
    /// when there's no selected iface or we're already in a non-Idle
    /// op state — pressing Enter twice during a Pending should not
    /// double-fire the worker.
    pub fn network_begin_dhcp(&mut self) -> Option<String> {
        if let Screen::Network {
            interfaces,
            selected,
            op,
            ..
        } = &mut self.screen
        {
            if !matches!(
                op,
                NetworkOp::Idle | NetworkOp::Failed { .. } | NetworkOp::Success { .. }
            ) {
                return None;
            }
            let iface = interfaces.get(*selected)?.name.clone();
            *op = NetworkOp::Pending {
                iface: iface.clone(),
                last_status: String::new(),
            };
            return Some(iface);
        }
        None
    }

    /// Update the Pending state's `last_status` text. Worker calls this
    /// via the main loop's `NetworkMsg::Progress` dispatch.
    pub fn network_progress(&mut self, status: String) {
        if let Screen::Network {
            op: NetworkOp::Pending { last_status, .. },
            ..
        } = &mut self.screen
        {
            *last_status = status;
        }
    }

    /// Apply the worker's terminal `NetworkMsg::Done` result. Flips
    /// the op state to Success or Failed and updates the highlighted
    /// iface row's `ipv4` so the table column reflects the new lease
    /// without forcing a refresh.
    pub fn network_finish_dhcp(
        &mut self,
        iface: String,
        result: Result<crate::network::NetworkLease, String>,
    ) {
        if let Screen::Network { interfaces, op, .. } = &mut self.screen {
            match result {
                Ok(lease) => {
                    if let Some(row) = interfaces.iter_mut().find(|i| i.name == iface) {
                        row.ipv4 = Some(lease.ipv4.clone());
                    }
                    // #655 PR-C: stash the lease so Catalog's `f`
                    // gate + the header banner can read it without
                    // walking screen history.
                    self.network_lease = Some(lease.clone());
                    *op = NetworkOp::Success { iface, lease };
                }
                Err(err) => {
                    *op = NetworkOp::Failed { iface, err };
                }
            }
        }
    }

    /// Open the [`Screen::Catalog`] overlay. Caller passes the
    /// catalog slice (almost always [`aegis_catalog::CATALOG`] but
    /// tests inject a small fixture). Stores the current screen as
    /// `prior` so [`Self::cancel_catalog`] can restore it. (#655
    /// Phase 2B PR-C step 2.)
    pub fn enter_catalog(&mut self, entries: &'static [aegis_catalog::Entry]) {
        if matches!(self.screen, Screen::Catalog { .. }) {
            return;
        }
        let prior = std::mem::replace(&mut self.screen, Screen::Quitting);
        self.screen = Screen::Catalog {
            entries,
            selected: 0,
            scroll: 0,
            prior: Box::new(prior),
        };
    }

    /// Cancel the Catalog overlay, returning to the screen that was
    /// active when it was opened.
    pub fn cancel_catalog(&mut self) {
        if let Screen::Catalog { prior, .. } = std::mem::replace(&mut self.screen, Screen::Quitting)
        {
            self.screen = *prior;
        }
    }

    /// Move the Catalog cursor by `delta` (`-1` = up, `+1` = down).
    /// Clamps to `[0, entries.len() - 1]`.
    pub fn catalog_move_selection(&mut self, delta: isize) {
        if let Screen::Catalog {
            entries, selected, ..
        } = &mut self.screen
        {
            let len = entries.len();
            if len == 0 {
                *selected = 0;
                return;
            }
            let max = len.saturating_sub(1);
            let new = i64::try_from(*selected).unwrap_or(0)
                + i64::from(i32::try_from(delta).unwrap_or(0));
            let clamped = new.clamp(0, i64::try_from(max).unwrap_or(0));
            *selected = usize::try_from(clamped).unwrap_or(0);
        }
    }

    /// Open [`Screen::CatalogConfirm`] for the currently-selected
    /// entry, with `free_bytes` measured from `statvfs` at
    /// confirm-open time. Returns the entry pointer iff the
    /// transition fired (caller may use this to reset progress
    /// renderers, etc.). Returns `None` when there's no selected
    /// entry (empty catalog) or we're not in [`Screen::Catalog`].
    pub fn catalog_open_confirm(
        &mut self,
        free_bytes: u64,
    ) -> Option<&'static aegis_catalog::Entry> {
        let Screen::Catalog {
            entries, selected, ..
        } = &self.screen
        else {
            return None;
        };
        let entry = entries.get(*selected)?;
        let prior = std::mem::replace(&mut self.screen, Screen::Quitting);
        self.screen = Screen::CatalogConfirm {
            entry,
            free_bytes,
            op: CatalogOp::Idle,
            prior: Box::new(prior),
        };
        Some(entry)
    }

    /// Cancel a [`Screen::CatalogConfirm`] back to [`Screen::Catalog`].
    /// Only fires when `op` is in a terminal state (`Idle`,
    /// `Success`, `Failed`) — pressing Esc mid-fetch is intentionally
    /// locked because the underlying fetch is uncancellable. Returns
    /// `true` iff the transition fired (caller can render a hint when
    /// the press was rejected).
    pub fn catalog_cancel_confirm(&mut self) -> bool {
        let cancellable = matches!(
            &self.screen,
            Screen::CatalogConfirm {
                op: CatalogOp::Idle | CatalogOp::Success(_) | CatalogOp::Failed(_),
                ..
            }
        );
        if !cancellable {
            return false;
        }
        if let Screen::CatalogConfirm { prior, .. } =
            std::mem::replace(&mut self.screen, Screen::Quitting)
        {
            self.screen = *prior;
            return true;
        }
        false
    }

    /// Mark the `CatalogConfirm` fetch as in-flight. Returns the
    /// entry pointer iff the transition fired (caller spawns the
    /// worker). Returns `None` when there's no [`Screen::CatalogConfirm`]
    /// active or `op` is in a non-resumable state.
    pub fn catalog_begin_fetch(&mut self) -> Option<&'static aegis_catalog::Entry> {
        let Screen::CatalogConfirm { entry, op, .. } = &mut self.screen else {
            return None;
        };
        if !matches!(
            op,
            CatalogOp::Idle | CatalogOp::Failed(_) | CatalogOp::Success(_)
        ) {
            return None;
        }
        *op = CatalogOp::Connecting;
        Some(*entry)
    }

    /// Block a fetch attempt because the requested ISO won't fit on
    /// the data partition. Flips `op` straight to `Failed` with an
    /// operator-readable message; caller of [`Self::catalog_begin_fetch`]
    /// is expected to call this on the size-precheck miss. Returns
    /// `true` iff the screen was [`Screen::CatalogConfirm`].
    pub fn catalog_block_for_disk_space(&mut self, message: String) -> bool {
        let Screen::CatalogConfirm { op, .. } = &mut self.screen else {
            return false;
        };
        *op = CatalogOp::Failed(message);
        true
    }

    /// Translate a [`aegis_fetch::FetchEvent`] into a
    /// [`Screen::CatalogConfirm`] op-state transition. Worker thread
    /// pushes events through the channel; the main loop calls this.
    /// Terminal events (`FetchEvent::Done`) are intentionally
    /// ignored here — the worker also sends a separate `FetchMsg::Done`
    /// with the typed result, and that's the canonical path to
    /// Success/Failed.
    pub fn catalog_progress(&mut self, ev: &aegis_fetch::FetchEvent) {
        let Screen::CatalogConfirm { op, .. } = &mut self.screen else {
            return;
        };
        if matches!(op, CatalogOp::Success(_) | CatalogOp::Failed(_)) {
            return;
        }
        *op = match ev {
            aegis_fetch::FetchEvent::Connecting => CatalogOp::Connecting,
            aegis_fetch::FetchEvent::Downloading(p) => CatalogOp::Downloading {
                bytes: p.bytes,
                total: p.total,
            },
            aegis_fetch::FetchEvent::VerifyingHash => CatalogOp::VerifyingHash,
            aegis_fetch::FetchEvent::VerifyingSig => CatalogOp::VerifyingSig,
            aegis_fetch::FetchEvent::Done(_) => return,
        };
    }

    /// Apply the worker's terminal `FetchMsg::Done` result. Flips
    /// the op state to Success or Failed.
    pub fn catalog_finish_fetch(&mut self, result: Result<aegis_fetch::FetchOutcome, String>) {
        let Screen::CatalogConfirm { op, .. } = &mut self.screen else {
            return;
        };
        *op = match result {
            Ok(outcome) => CatalogOp::Success(outcome),
            Err(err) => CatalogOp::Failed(err),
        };
    }

    /// Open the help overlay over the current screen. (#85)
    pub fn open_help(&mut self) {
        // Don't stack — if already in Help, do nothing.
        if matches!(self.screen, Screen::Help { .. }) {
            return;
        }
        let prior = std::mem::replace(&mut self.screen, Screen::Quitting);
        self.screen = Screen::Help {
            prior: Box::new(prior),
        };
    }

    /// Dismiss the help overlay and restore the prior screen.
    pub fn close_help(&mut self) {
        if let Screen::Help { prior } = std::mem::replace(&mut self.screen, Screen::Quitting) {
            self.screen = *prior;
        }
    }

    /// Open the quit-confirmation prompt over the current screen.
    /// Idempotent — re-pressing `q` in the prompt does nothing. (#85)
    pub fn request_quit(&mut self) {
        if matches!(self.screen, Screen::ConfirmQuit { .. } | Screen::Quitting) {
            return;
        }
        let prior = std::mem::replace(&mut self.screen, Screen::Quitting);
        self.screen = Screen::ConfirmQuit {
            prior: Box::new(prior),
        };
    }

    /// Dismiss the quit prompt without exiting.
    pub fn cancel_quit(&mut self) {
        if let Screen::ConfirmQuit { prior } = std::mem::replace(&mut self.screen, Screen::Quitting)
        {
            self.screen = *prior;
        }
    }

    /// Coarse trust classification for the Confirm screen's verdict
    /// line AND for deciding whether [`Self::enter_trust_challenge`]
    /// fires before a kexec. Mirrors the `TrustVerdict` enum in
    /// render.rs but kept here so state-machine tests don't need to
    /// depend on the UI layer. (#93)
    #[must_use]
    pub fn is_degraded_trust(&self, idx: usize) -> bool {
        let Some(iso) = self.isos.get(idx) else {
            return false;
        };
        // GREEN if hash OR signature is verified.
        let verified = matches!(
            iso.signature_verification,
            SignatureVerification::Verified { .. }
        ) || matches!(iso.hash_verification, HashVerification::Verified { .. });
        !verified
    }

    /// Enter the typed-confirmation challenge screen for a degraded
    /// trust state. (#93)
    pub fn enter_trust_challenge(&mut self, idx: usize) {
        self.screen = Screen::TrustChallenge {
            selected: idx,
            buffer: String::new(),
        };
    }

    /// Character entry for the trust challenge. Returns true iff the
    /// buffer now equals the expected token "boot" — caller then
    /// proceeds to kexec.
    pub fn trust_challenge_push(&mut self, c: char) -> bool {
        if let Screen::TrustChallenge { buffer, .. } = &mut self.screen {
            buffer.push(c);
            return buffer == "boot";
        }
        false
    }

    /// Backspace in the trust challenge buffer.
    pub fn trust_challenge_backspace(&mut self) {
        if let Screen::TrustChallenge { buffer, .. } = &mut self.screen {
            buffer.pop();
        }
    }

    /// Cancel the trust challenge and return to Confirm.
    pub fn trust_challenge_cancel(&mut self) {
        if let Screen::TrustChallenge { selected, .. } = self.screen {
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Open the Verifying screen for the ISO at real index `idx`.
    /// Caller (main.rs) spawns the worker thread that will feed
    /// progress ticks via [`Self::verify_tick`] and completion via
    /// [`Self::verify_finish`]. (#89)
    pub fn begin_verify(&mut self, idx: usize) {
        if self.isos.get(idx).is_some() {
            self.screen = Screen::Verifying {
                selected: idx,
                bytes: 0,
                total: 0,
                result: None,
            };
        }
    }

    /// Progress update from the verify worker.
    pub fn verify_tick(&mut self, new_bytes: u64, new_total: u64) {
        if let Screen::Verifying { bytes, total, .. } = &mut self.screen {
            *bytes = new_bytes;
            *total = new_total;
        }
    }

    /// Return a reference to the ISO currently under verify-now, if
    /// the screen is `Verifying`. Used by main.rs to write the
    /// audit-log line (#548) before `verify_finish` transitions
    /// state away from the Verifying screen.
    #[must_use]
    pub fn iso_being_verified(&self) -> Option<&DiscoveredIso> {
        if let Screen::Verifying { selected, .. } = self.screen {
            self.isos.get(selected)
        } else {
            None
        }
    }

    /// Worker finished. Update the ISO's `hash_verification` in place
    /// and transition back to the Confirm screen on the same ISO so
    /// the operator sees the refreshed verdict. (#89)
    pub fn verify_finish(&mut self, outcome: HashVerification) {
        if let Screen::Verifying { selected, .. } = self.screen {
            if let Some(iso) = self.isos.get_mut(selected) {
                iso.hash_verification = outcome;
            }
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Produce a plain-text evidence snapshot of the current Error
    /// screen suitable for writing to the `AEGIS_ISOS` data partition.
    /// memtest86+-style "one frame = one bug report," serialized.
    /// (#92)
    #[must_use]
    pub fn error_evidence_text(&self) -> Option<String> {
        use std::fmt::Write as _;
        let (msg, remedy, return_to) = match &self.screen {
            Screen::Error {
                message,
                remedy,
                return_to,
            } => (message.clone(), remedy.clone(), *return_to),
            _ => return None,
        };
        let iso = self.isos.get(return_to)?;
        let cmdline = self.effective_cmdline(return_to);
        let mut body = String::new();
        body.push_str("aegis-boot kexec-failure evidence\n");
        body.push_str("=================================\n\n");
        let _ = writeln!(body, "Diagnostic: {msg}");
        if let Some(r) = remedy {
            let _ = writeln!(body, "Remedy:     {r}");
        }
        body.push('\n');
        let _ = writeln!(
            body,
            "Version:    aegis-boot v{}",
            env!("CARGO_PKG_VERSION")
        );
        let _ = writeln!(
            body,
            "SB / TPM:   {}  ·  {}",
            self.secure_boot.summary(),
            self.tpm.summary()
        );
        let _ = writeln!(body, "ISO label:  {}", iso.label);
        let _ = writeln!(body, "ISO path:   {}", iso.iso_path.display());
        let _ = writeln!(body, "Size:       {:?}", iso.size_bytes);
        let _ = writeln!(body, "Distribution: {:?}", iso.distribution);
        let _ = writeln!(body, "Quirks:     {:?}", iso.quirks);
        let _ = writeln!(body, "Hash state: {:?}", iso.hash_verification);
        let _ = writeln!(body, "Sig state:  {:?}", iso.signature_verification);
        let cmdline_display = if cmdline.is_empty() {
            "(none)"
        } else {
            &cmdline
        };
        let _ = writeln!(body, "Cmdline:    {cmdline_display}");
        Some(body)
    }

    /// Cancel an in-progress verification (Esc). Dismisses the
    /// Verifying screen without updating the iso. The worker thread
    /// continues in the background; its result is discarded.
    pub fn cancel_verify(&mut self) {
        if let Screen::Verifying { selected, .. } = self.screen {
            self.screen = Screen::Confirm { selected };
        }
    }

    /// Confirm exit — only call from `ConfirmQuit` screen. Direct exit.
    pub fn confirm_quit(&mut self) {
        self.screen = Screen::Quitting;
    }

    /// Legacy direct quit — kept for tests that exercise the terminal
    /// state but no longer wired to a key. New code should call
    /// [`Self::request_quit`] instead so the operator gets a confirmation.
    #[cfg(test)]
    pub fn quit(&mut self) {
        self.screen = Screen::Quitting;
    }

    /// Currently highlighted/selected ISO, if the screen has one.
    #[must_use]
    #[cfg(test)]
    pub fn selected_iso(&self) -> Option<&DiscoveredIso> {
        let selected = match &self.screen {
            Screen::List { selected }
            | Screen::Confirm { selected }
            | Screen::EditCmdline { selected, .. } => *selected,
            _ => return None,
        };
        self.isos.get(selected)
    }
}

/// Find the byte offset of the nearest char boundary before `cursor`.
///
/// UTF-8 chars are 1-4 bytes; step back 1 byte at a time looking for the
/// previous boundary. Walks up to 4 bytes in the worst case.
fn prev_char_boundary(buffer: &str, cursor: usize) -> usize {
    (1..=cursor.min(4))
        .find(|step| buffer.is_char_boundary(cursor - step))
        .map_or(0, |step| cursor - step)
}

/// Build the MOK enrollment remedy text as a three-step walkthrough.
/// Split from a single dense paragraph into numbered steps + a
/// firmware-boot-key hint table so operators can execute without
/// context-switching to external docs. (#136)
///
/// When the ISO has a discoverable sibling key file, step 1 becomes a
/// literal copy-paste `mokutil --import` command. When it doesn't, step 1
/// tells the operator how to place the key so the next run will name it.
fn build_mokutil_remedy(iso: Option<&DiscoveredIso>) -> String {
    let key_hint = iso.and_then(|iso| find_sibling_key(&iso.iso_path));
    let step_one = match key_hint {
        Some(ref key_path) => format!(
            "STEP 1/3 — Enroll the key (run on the host, not here):\n  \
             sudo mokutil --import {}\n  \
             (mokutil will prompt you to set a temporary password — you'll \
             need this exact password in step 2.)",
            key_path.display()
        ),
        None => "STEP 1/3 — Get the signing key, then enroll it (run on the host, not here):\n  \
                 • Find the distro's signing public key (usually a `.pub`, `.key`, or\n    \
                 `.der` file on the distro's download page).\n  \
                 • Place it alongside the ISO using any of these filenames:\n    \
                 `<iso>.pub`, `<iso>.key`, `<iso>.der` — aegis-boot will then\n    \
                 generate the exact `sudo mokutil --import <path>` command\n    \
                 for you on the next kexec attempt.\n  \
                 • After running `mokutil --import`, mokutil will prompt you to\n    \
                 set a temporary password. You'll need it in step 2."
            .to_string(),
    };
    format!(
        "{step_one}\n\
         \n\
         STEP 2/3 — Reboot and complete enrollment in MOK Manager:\n  \
         • Reboot the machine.\n  \
         • The firmware will show a blue-on-black screen titled\n    \
         \"Perform MOK management\" before the normal boot.\n  \
         • Choose \"Enroll MOK\" → \"Continue\" → \"Yes\" → enter the\n    \
         temporary password from step 1 → \"Reboot\".\n  \
         • If the machine skips straight past the blue screen, power-cycle\n    \
         within 10 seconds of the firmware splash; the MOK window is short.\n\
         \n\
         STEP 3/3 — Boot back into aegis-boot and retry:\n  \
         • Tap your firmware's boot-menu key at power-on:\n    \
         Lenovo/Dell: F12    HP: F9    ASUS: F8    MSI: F11    Apple: Option\n  \
         • Re-select the aegis-boot USB → re-select this ISO → Enter.\n  \
         • The kernel's signature now verifies against the MOK you enrolled.\n\
         \n\
         Do NOT disable Secure Boot. Do NOT enroll a global MOK (that's\n\
         Ventoy's approach and defeats the chain of trust). See\n\
         docs/UNSIGNED_KERNEL.md for the full guide (#126)."
    )
}

/// Look for a sibling key file alongside the ISO. Accepts `.pub`, `.key`,
/// and `.der` extensions — the most common formats for detached signing
/// keys published by distros.
fn find_sibling_key(iso_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let base = iso_path.file_stem()?;
    let dir = iso_path.parent()?;
    for ext in ["pub", "key", "der"] {
        let candidate = dir.join(format!("{}.{ext}", base.to_string_lossy()));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // Also try full-filename extensions: ubuntu.iso.pub alongside ubuntu.iso
    let fname = iso_path.file_name()?;
    for ext in ["pub", "key", "der"] {
        let candidate = dir.join(format!("{}.{ext}", fname.to_string_lossy()));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Map a [`KexecError`] to (user-facing message, optional remedy hint).
///
/// Diagnostics are deliberately specific so the TUI never shows a generic
/// "kexec failed" — see ADR 0001's commitment to "no black screens."
///
/// For context-free error mapping. The TUI-level variant
/// [`error_diagnostic_with_iso`] augments the [`KexecError::SignatureRejected`]
/// remedy with a specific `mokutil --import` command when the ISO has a
/// sibling key file.
#[must_use]
#[cfg_attr(not(test), allow(dead_code))]
pub fn error_diagnostic(err: &KexecError) -> (String, Option<String>) {
    error_diagnostic_with_iso(err, None)
}

/// Same as [`error_diagnostic`] but enriches the remedy with ISO-specific
/// context when available. For `SignatureRejected`, if the caller passes
/// in the ISO that triggered the error, we look for sibling key files
/// (`<iso>.pub`, `<iso>.key`, `<iso>.der`) and embed the exact
/// `mokutil --import` command — removing the "which file do I enroll?"
/// guessing game from the recovery flow.
#[must_use]
pub fn error_diagnostic_with_iso(
    err: &KexecError,
    iso: Option<&DiscoveredIso>,
) -> (String, Option<String>) {
    match err {
        KexecError::SignatureRejected => (
            "Kernel signature verification failed (KEXEC_SIG)".to_string(),
            Some(build_mokutil_remedy(iso)),
        ),
        KexecError::LockdownRefused => (
            "Operation refused by kernel lockdown".to_string(),
            Some(
                "This is by design under enforcing Secure Boot. The selected kernel cannot be \
                 booted on this system without an enrolled signing key."
                    .to_string(),
            ),
        ),
        KexecError::UnsupportedImage => (
            "Kernel image format not recognized".to_string(),
            Some("The ISO's kernel is not a valid bzImage; this ISO is not supported.".to_string()),
        ),
        KexecError::AlreadyLoaded => (
            "Another kexec image is already loaded".to_string(),
            Some(
                "Another kexec image is staged in this kernel. Run `kexec -u` to unload, \
                 or reboot the rescue stick fresh before retrying."
                    .to_string(),
            ),
        ),
        KexecError::ImageTooLarge => (
            "Kernel image too large for kexec".to_string(),
            Some(
                "The ISO's kernel exceeds the kexec_file_load size limit. Try a smaller \
                 kernel variant (e.g. non-debug, non-bigmem build) on the same ISO."
                    .to_string(),
            ),
        ),
        KexecError::PermissionDenied => (
            "Permission denied opening kernel or initrd".to_string(),
            Some(
                "The operator could not read the kernel or initrd file. Check the ISO is \
                 mounted and the rescue process has read access."
                    .to_string(),
            ),
        ),
        KexecError::OutOfMemory => (
            "Not enough memory to stage the kexec image".to_string(),
            Some(
                "The kernel could not allocate enough memory to load this image. Reboot \
                 to a fresh rescue state and try again."
                    .to_string(),
            ),
        ),
        KexecError::Io(io) => (
            format!("System error: {io}"),
            io.raw_os_error()
                .map(|errno| format!("Underlying errno = {errno}.")),
        ),
        KexecError::InvalidPath(p) => (
            format!("Invalid path supplied to kexec: {}", p.display()),
            None,
        ),
        KexecError::Unsupported => (
            "kexec is only available on Linux".to_string(),
            Some(
                "rescue-tui is meant to run inside the signed Linux rescue initramfs.".to_string(),
            ),
        ),
    }
}

/// Render a one-line summary of an ISO's quirks for the list view.
/// Returns an empty string if there are none.
#[must_use]
pub fn quirks_summary(iso: &DiscoveredIso) -> String {
    if iso.quirks.is_empty() {
        return String::new();
    }
    let parts: Vec<&'static str> = iso
        .quirks
        .iter()
        .map(|q| match q {
            Quirk::UnsignedKernel => "unsigned-kernel",
            Quirk::BiosOnly => "bios-only",
            Quirk::RequiresWholeDeviceWrite => "needs-dd",
            Quirk::CrossDistroKexecRefused => "cross-distro-refused",
            Quirk::NotKexecBootable => "not-kexec-bootable",
        })
        .collect();
    format!("[{}]", parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iso_probe::Distribution;
    use std::path::PathBuf;

    fn fake_iso(label: &str) -> DiscoveredIso {
        DiscoveredIso {
            iso_path: PathBuf::from(format!("/run/media/{label}.iso")),
            label: label.to_string(),
            distribution: Distribution::Debian,
            kernel: PathBuf::from("casper/vmlinuz"),
            initrd: Some(PathBuf::from("casper/initrd")),
            cmdline: Some("boot=casper".to_string()),
            quirks: vec![],
            hash_verification: iso_probe::HashVerification::NotPresent,
            signature_verification: iso_probe::SignatureVerification::NotPresent,
            size_bytes: Some(1_500_000_000),
            contains_installer: false,
            pretty_name: None,
            sidecar: None,
        }
    }

    #[test]
    fn empty_list_blocks_confirmation_and_movement() {
        let mut s = AppState::new(vec![]);
        s.move_selection(1);
        assert_eq!(s.screen, Screen::List { selected: 0 });
        s.confirm_selection();
        assert_eq!(s.screen, Screen::List { selected: 0 });
        assert!(s.selected_iso().is_none());
    }

    #[test]
    fn movement_clamps_at_bounds() {
        // 3 ISOs + 1 rescue-shell row = 4 entries; max cursor = 3.
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_selection(-5);
        assert_eq!(s.screen, Screen::List { selected: 0 });
        s.move_selection(99);
        assert_eq!(s.screen, Screen::List { selected: 3 });
    }

    #[test]
    fn confirm_then_cancel_returns_to_list_with_same_selection() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        s.move_selection(1);
        s.confirm_selection();
        assert_eq!(s.screen, Screen::Confirm { selected: 1 });
        s.cancel_confirmation();
        assert_eq!(s.screen, Screen::List { selected: 1 });
    }

    fn unwrap_remedy(remedy: Option<String>) -> String {
        remedy.unwrap_or_else(|| panic!("expected remedy to be Some"))
    }

    #[test]
    fn signature_rejected_diagnostic_includes_mokutil_hint() {
        let (msg, remedy) = error_diagnostic(&KexecError::SignatureRejected);
        assert!(msg.to_lowercase().contains("signature"));
        let r = unwrap_remedy(remedy);
        assert!(r.contains("mokutil"));
        // ADR 0001: must not *suggest* disabling SB. Allow "Do NOT disable" phrasing.
        let lc = r.to_lowercase();
        assert!(!lc.contains("disable secure boot") || lc.contains("not disable"));
    }

    #[test]
    fn signature_rejected_without_iso_gives_generic_guidance() {
        // No ISO context — remedy should tell user where to put the key
        // so future attempts can auto-suggest.
        let (_, remedy) = error_diagnostic_with_iso(&KexecError::SignatureRejected, None);
        let r = unwrap_remedy(remedy);
        assert!(r.contains("<iso>.pub") || r.contains(".pub"));
        // Remedy says "Do NOT disable Secure Boot" — literal phrase is
        // fine; the ADR 0001 commitment is no *suggestion* to disable.
        let lc = r.to_lowercase();
        assert!(!lc.contains("disable secure boot") || lc.contains("not disable"));
    }

    #[test]
    fn signature_rejected_remedy_is_three_step_walkthrough() {
        // Remedy must be structured as STEP 1/3, STEP 2/3, STEP 3/3 so
        // operators can execute it without reading a dense paragraph.
        // Also must mention MOK Manager's blue-screen cue and list
        // firmware boot-menu keys for the top vendors. (#136)
        let (_, remedy) = error_diagnostic_with_iso(&KexecError::SignatureRejected, None);
        let r = unwrap_remedy(remedy);
        assert!(r.contains("STEP 1/3"), "missing STEP 1/3 header in: {r}");
        assert!(r.contains("STEP 2/3"), "missing STEP 2/3 header in: {r}");
        assert!(r.contains("STEP 3/3"), "missing STEP 3/3 header in: {r}");
        assert!(
            r.contains("MOK management") || r.contains("MOK Manager"),
            "remedy must reference MOK Manager by name so operators know what to expect on the reboot"
        );
        let lower = r.to_ascii_lowercase();
        assert!(
            lower.contains("f12") || lower.contains("f9") || lower.contains("f11"),
            "remedy must mention firmware boot-menu keys to close the 'how do I get back?' loop"
        );
    }

    #[test]
    fn signature_rejected_with_sibling_key_embeds_exact_command() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let iso_path = dir.path().join("test.iso");
        fs::write(&iso_path, b"dummy").unwrap_or_else(|e| panic!("write: {e}"));
        let key_path = dir.path().join("test.pub");
        fs::write(&key_path, b"pubkey").unwrap_or_else(|e| panic!("write key: {e}"));

        let iso = DiscoveredIso {
            iso_path: iso_path.clone(),
            label: "t".into(),
            distribution: iso_probe::Distribution::Debian,
            kernel: std::path::PathBuf::from("vmlinuz"),
            initrd: None,
            cmdline: None,
            quirks: vec![],
            hash_verification: iso_probe::HashVerification::NotPresent,
            signature_verification: iso_probe::SignatureVerification::NotPresent,
            size_bytes: Some(1_500_000_000),
            contains_installer: false,
            pretty_name: None,
            sidecar: None,
        };
        let (_, remedy) = error_diagnostic_with_iso(&KexecError::SignatureRejected, Some(&iso));
        let r = unwrap_remedy(remedy);
        assert!(r.contains("mokutil --import"));
        assert!(r.contains(&key_path.display().to_string()));
    }

    #[test]
    fn find_sibling_key_tries_stem_and_full_filename() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        // Case 1: .pub with stem (debian.pub alongside debian.iso)
        let iso_a = dir.path().join("debian.iso");
        fs::write(&iso_a, b"x").unwrap_or_else(|e| panic!("write: {e}"));
        let key_a = dir.path().join("debian.pub");
        fs::write(&key_a, b"k").unwrap_or_else(|e| panic!("write: {e}"));
        assert_eq!(find_sibling_key(&iso_a), Some(key_a));

        // Case 2: .iso.pub full-name style (ubuntu.iso.pub alongside ubuntu.iso)
        let iso_b = dir.path().join("ubuntu.iso");
        fs::write(&iso_b, b"y").unwrap_or_else(|e| panic!("write: {e}"));
        let key_b = dir.path().join("ubuntu.iso.pub");
        fs::write(&key_b, b"k").unwrap_or_else(|e| panic!("write: {e}"));
        assert_eq!(find_sibling_key(&iso_b), Some(key_b));

        // Case 3: no key
        let iso_c = dir.path().join("orphan.iso");
        fs::write(&iso_c, b"z").unwrap_or_else(|e| panic!("write: {e}"));
        assert!(find_sibling_key(&iso_c).is_none());
    }

    #[test]
    fn lockdown_diagnostic_does_not_suggest_disabling_sb() {
        let (msg, remedy) = error_diagnostic(&KexecError::LockdownRefused);
        assert!(msg.to_lowercase().contains("lockdown"));
        let r = unwrap_remedy(remedy);
        let lc = r.to_lowercase();
        assert!(!lc.contains("disable secure boot"));
    }

    #[test]
    fn io_diagnostic_preserves_errno() {
        let io = std::io::Error::from_raw_os_error(13);
        let err = KexecError::Io(io);
        let (msg, remedy) = error_diagnostic(&err);
        assert!(msg.contains("System error"));
        assert!(unwrap_remedy(remedy).contains("13"));
    }

    #[test]
    fn record_kexec_error_transitions_to_error_screen() {
        let mut s = AppState::new(vec![fake_iso("x")]);
        s.confirm_selection();
        s.record_kexec_error(&KexecError::SignatureRejected);
        assert!(matches!(s.screen, Screen::Error { .. }));
    }

    #[test]
    fn quit_transitions_to_quitting() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.quit();
        assert_eq!(s.screen, Screen::Quitting);
    }

    // --- Tier 1 UX (#85) -----------------------------------------------

    #[test]
    fn request_quit_opens_confirm_overlay() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.request_quit();
        assert!(matches!(s.screen, Screen::ConfirmQuit { .. }));
    }

    #[test]
    fn cancel_quit_restores_prior_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.request_quit();
        s.cancel_quit();
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
    }

    #[test]
    fn confirm_quit_exits() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.request_quit();
        s.confirm_quit();
        assert_eq!(s.screen, Screen::Quitting);
    }

    #[test]
    fn open_help_overlays_prior_screen() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        s.move_selection(1);
        s.open_help();
        assert!(matches!(s.screen, Screen::Help { .. }));
        s.close_help();
        assert!(matches!(s.screen, Screen::List { selected: 1 }));
    }

    #[test]
    fn move_to_first_jumps_to_zero() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_selection(2);
        assert_eq!(s.screen, Screen::List { selected: 2 });
        s.move_to_first();
        assert_eq!(s.screen, Screen::List { selected: 0 });
    }

    #[test]
    fn move_to_last_jumps_to_max() {
        // 3 ISOs + 1 rescue-shell row = last cursor is 3.
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_to_last();
        assert_eq!(s.screen, Screen::List { selected: 3 });
    }

    #[test]
    fn record_kexec_error_preserves_failed_selection() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.move_selection(1);
        s.confirm_selection();
        s.record_kexec_error(&KexecError::SignatureRejected);
        match s.screen {
            Screen::Error { return_to, .. } => assert_eq!(return_to, 1),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // --- Tier 2 UX (#85) -----------------------------------------------

    #[test]
    fn visible_indices_no_filter_returns_all_sorted_by_name() {
        let s = AppState::new(vec![
            fake_iso("zebra"),
            fake_iso("alpha"),
            fake_iso("mango"),
        ]);
        let v = s.visible_indices();
        assert_eq!(v.len(), 3);
        // SortOrder::Name is default — labels must come back A..Z.
        let labels: Vec<&str> = v.iter().map(|&i| s.isos[i].label.as_str()).collect();
        assert_eq!(labels, ["alpha", "mango", "zebra"]);
    }

    #[test]
    fn filter_substring_case_insensitive() {
        let mut s = AppState::new(vec![
            fake_iso("Ubuntu-24.04"),
            fake_iso("fedora-40"),
            fake_iso("debian-12"),
        ]);
        s.filter = "DEB".to_string();
        let labels: Vec<&str> = s
            .visible_indices()
            .iter()
            .map(|&i| s.isos[i].label.as_str())
            .collect();
        assert_eq!(labels, ["debian-12"]);
    }

    #[test]
    fn cycle_sort_visits_all_orders() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.sort_order, SortOrder::Name);
        s.cycle_sort();
        assert_eq!(s.sort_order, SortOrder::SizeDesc);
        s.cycle_sort();
        assert_eq!(s.sort_order, SortOrder::Distro);
        s.cycle_sort();
        assert_eq!(s.sort_order, SortOrder::Name);
    }

    #[test]
    fn filter_editing_typing_pins_cursor_to_zero() {
        let mut s = AppState::new(vec![fake_iso("aaa"), fake_iso("bbb"), fake_iso("ccc")]);
        s.move_selection(2);
        s.open_filter();
        s.filter_push('b');
        // After typing, cursor should be 0 (first match: bbb).
        assert_eq!(s.screen, Screen::List { selected: 0 });
    }

    #[test]
    fn filter_cancel_clears_query_and_resets_cursor() {
        let mut s = AppState::new(vec![fake_iso("aaa"), fake_iso("bbb")]);
        s.open_filter();
        s.filter_push('z'); // matches nothing
        s.filter_cancel();
        assert!(s.filter.is_empty());
        assert!(!s.filter_editing);
        assert_eq!(s.visible_indices().len(), 2);
    }

    #[test]
    fn confirm_selection_translates_visible_cursor_to_real_index() {
        // Filter so only the second iso is visible. Cursor=0 in the
        // visible view should map to real index 1.
        let mut s = AppState::new(vec![fake_iso("ubuntu"), fake_iso("debian")]);
        s.filter = "debian".to_string();
        s.confirm_selection();
        assert_eq!(s.screen, Screen::Confirm { selected: 1 });
    }

    // ---- BlockedToast (#546 UX T3C) -----------------------------------

    fn fake_failed_iso(reason: &str) -> iso_probe::FailedIso {
        iso_probe::FailedIso {
            iso_path: std::path::PathBuf::from("/run/media/AEGIS_ISOS/broken.iso"),
            reason: reason.to_string(),
            kind: iso_probe::FailureKind::NoBootEntries,
        }
    }

    #[test]
    fn confirm_selection_over_failed_iso_opens_blocked_toast() {
        // #546: tier-4 rows (FailedIso) used to refuse Enter silently.
        // Now they route to a BlockedToast popup whose message includes
        // the parse-failed reason so the operator gets immediate
        // feedback instead of "did Enter even register?"
        let mut s =
            AppState::new(vec![]).with_failed_isos(vec![fake_failed_iso("kernel/initrd missing")]);
        // Cursor at 0 = the FailedIso row (no real ISOs, so it's first).
        s.screen = Screen::List { selected: 0 };
        s.confirm_selection();
        match &s.screen {
            Screen::BlockedToast { message, return_to } => {
                assert!(
                    message.contains("kernel/initrd missing"),
                    "toast message must include the parse-failed reason, got: {message}"
                );
                assert!(message.starts_with("Cannot boot:"), "got: {message}");
                assert_eq!(*return_to, 0, "cursor preserved for dismissal");
            }
            other => panic!("expected BlockedToast, got {other:?}"),
        }
    }

    #[test]
    fn dismiss_blocked_toast_returns_to_list_at_original_cursor() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        s.screen = Screen::BlockedToast {
            message: "Cannot boot: parse failed — synthetic test reason".to_string(),
            return_to: 1,
        };
        s.dismiss_blocked_toast();
        assert_eq!(s.screen, Screen::List { selected: 1 });
    }

    #[test]
    fn dismiss_blocked_toast_no_op_outside_toast_screen() {
        // Defensive: dismiss called from any non-toast screen must not
        // mutate state (would otherwise mask other UI bugs).
        let mut s = AppState::new(vec![fake_iso("a")]);
        let before = s.screen.clone();
        s.dismiss_blocked_toast();
        assert_eq!(s.screen, before);
    }

    #[test]
    fn confirm_selection_iso_row_unchanged_by_blocked_toast_addition() {
        // Regression guard: the FailedIso branch I added must not
        // change the existing ViewEntry::Iso → Screen::Confirm path.
        let mut s = AppState::new(vec![fake_iso("ubuntu")]);
        s.confirm_selection();
        assert_eq!(s.screen, Screen::Confirm { selected: 0 });
    }

    // --- rescue-shell entry (#90) -------------------------------------

    #[test]
    fn rescue_shell_entry_always_last_even_with_no_isos() {
        let s = AppState::new(vec![]);
        let entries = s.visible_entries();
        assert_eq!(entries, vec![ViewEntry::RescueShell]);
    }

    #[test]
    fn rescue_shell_entry_appended_after_isos() {
        let s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        let entries = s.visible_entries();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[2], ViewEntry::RescueShell));
    }

    #[test]
    fn rescue_shell_entry_survives_filter_with_no_iso_matches() {
        let mut s = AppState::new(vec![fake_iso("ubuntu")]);
        s.filter = "nope".to_string();
        assert_eq!(s.visible_entries(), vec![ViewEntry::RescueShell]);
    }

    #[test]
    fn is_shell_selected_true_when_cursor_on_shell_row() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.move_to_last();
        assert!(s.is_shell_selected());
    }

    #[test]
    fn is_shell_selected_false_when_cursor_on_iso_row() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.move_to_first();
        assert!(!s.is_shell_selected());
    }

    // --- typed trust challenge (#93) ----------------------------------

    #[test]
    fn enter_trust_challenge_opens_screen_with_empty_buffer() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        assert!(matches!(
            s.screen,
            Screen::TrustChallenge {
                selected: 0,
                ref buffer,
            } if buffer.is_empty()
        ));
    }

    #[test]
    fn trust_challenge_push_returns_true_at_boot_string() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        assert!(!s.trust_challenge_push('b'));
        assert!(!s.trust_challenge_push('o'));
        assert!(!s.trust_challenge_push('o'));
        assert!(s.trust_challenge_push('t'));
    }

    #[test]
    fn trust_challenge_backspace_rolls_back() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        s.trust_challenge_push('b');
        s.trust_challenge_push('x');
        s.trust_challenge_backspace();
        // 'b' alone is not "boot" yet.
        assert!(!matches!(
            &s.screen,
            Screen::TrustChallenge { buffer, .. } if buffer == "boot"
        ));
    }

    #[test]
    fn trust_challenge_cancel_returns_to_confirm() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_trust_challenge(0);
        s.trust_challenge_cancel();
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
    }

    #[test]
    fn is_degraded_trust_true_when_no_verification() {
        let s = AppState::new(vec![fake_iso("a")]);
        // fake_iso() has NotPresent for both hash and sig → degraded.
        assert!(s.is_degraded_trust(0));
    }

    #[test]
    fn is_degraded_trust_false_when_hash_verified() {
        let mut iso = fake_iso("a");
        iso.hash_verification = HashVerification::Verified {
            digest: "abc".to_string(),
            source: "/x".to_string(),
        };
        let s = AppState::new(vec![iso]);
        assert!(!s.is_degraded_trust(0));
    }

    // --- one-frame error evidence (#92) -------------------------------

    #[test]
    fn error_evidence_text_populated_after_kexec_error() {
        let mut s = AppState::new(vec![fake_iso("ubuntu-24.04")]);
        s.confirm_selection();
        s.record_kexec_error(&KexecError::SignatureRejected);
        let Some(text) = s.error_evidence_text() else {
            panic!("evidence populated on Error");
        };
        assert!(text.contains("aegis-boot kexec-failure evidence"));
        assert!(text.contains("ubuntu-24.04"));
        assert!(text.contains("Diagnostic:"));
    }

    #[test]
    fn error_evidence_text_none_when_not_error_screen() {
        let s = AppState::new(vec![fake_iso("a")]);
        assert!(s.error_evidence_text().is_none());
    }

    // --- verify-now (#89) ---------------------------------------------

    #[test]
    fn begin_verify_opens_verifying_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.begin_verify(0);
        assert!(matches!(
            s.screen,
            Screen::Verifying {
                selected: 0,
                bytes: 0,
                ..
            }
        ));
    }

    #[test]
    fn verify_tick_updates_progress() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.begin_verify(0);
        s.verify_tick(1024, 4096);
        match s.screen {
            Screen::Verifying { bytes, total, .. } => {
                assert_eq!(bytes, 1024);
                assert_eq!(total, 4096);
            }
            _ => panic!("expected Verifying"),
        }
    }

    #[test]
    fn verify_finish_updates_iso_and_returns_to_confirm() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.begin_verify(0);
        let outcome = HashVerification::Verified {
            digest: "abc123".to_string(),
            source: "fake".to_string(),
        };
        s.verify_finish(outcome.clone());
        assert_eq!(s.isos[0].hash_verification, outcome);
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
    }

    #[test]
    fn cancel_verify_returns_to_confirm_without_updating_iso() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        let original = s.isos[0].hash_verification.clone();
        s.begin_verify(0);
        s.cancel_verify();
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
        assert_eq!(s.isos[0].hash_verification, original);
    }

    /// #602: a fresh `AppState` has no audit warning. Set + clear are
    /// the two mutators `main.rs` uses; both must round-trip.
    #[test]
    fn audit_warning_default_none_and_set_clear_round_trip() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert!(s.audit_warning.is_none());

        s.set_audit_warning("audit log write failed (disk full) — verdict shown but not persisted");
        assert_eq!(
            s.audit_warning.as_deref(),
            Some("audit log write failed (disk full) — verdict shown but not persisted")
        );

        s.clear_audit_warning();
        assert!(s.audit_warning.is_none());
    }

    /// #602: the audit warning must NOT affect the kexec gate. A green
    /// ISO with a stale audit failure banner can still kexec — the
    /// banner is informational, not a block.
    #[test]
    fn audit_warning_does_not_affect_kexec_gate() {
        let mut s = AppState::new(vec![fake_iso("clean")]);
        assert!(!s.is_kexec_blocked(0));

        s.set_audit_warning("audit log write failed (read-only fs)");
        assert!(
            !s.is_kexec_blocked(0),
            "audit warning is informational and must not gate kexec"
        );
    }

    // ---- #347 consent screen + per-session consent ---------------------

    fn fake_iso_with_installer_flag(name: &str, contains_installer: bool) -> DiscoveredIso {
        let mut iso = fake_iso(name);
        iso.contains_installer = contains_installer;
        iso
    }

    /// #347: clean ISO (no installer warning, no quirks) needs no
    /// consent screen. The Confirm-Enter handler proceeds straight to
    /// the trust-challenge / kexec dispatch chain.
    #[test]
    fn consent_not_required_for_clean_iso() {
        let s = AppState::new(vec![fake_iso("clean")]);
        assert!(
            s.consent_required_for(0).is_none(),
            "no consent for clean ISO"
        );
    }

    /// #347: an ISO carrying an installer (#131 `contains_installer`
    /// flag) requires consent before kexec can proceed — that's the
    /// "installer can erase disks" gate.
    #[test]
    fn consent_required_for_installer_iso_pre_grant() {
        let s = AppState::new(vec![fake_iso_with_installer_flag("ubuntu-server", true)]);
        let Some(kind) = s.consent_required_for(0) else {
            panic!("installer ISO must require consent");
        };
        assert_eq!(kind, ConsentKind::InstallerCanEraseDisks);
    }

    /// #347: per-session consent — once granted, subsequent
    /// installer-ISO selections in the same session do NOT re-trigger
    /// the consent gate. Resets on rescue-tui restart.
    #[test]
    fn consent_required_returns_none_after_session_grant() {
        let mut s = AppState::new(vec![fake_iso_with_installer_flag("ubuntu-server", true)]);
        s.session_consent = true;
        assert!(
            s.consent_required_for(0).is_none(),
            "session_consent should short-circuit the gate"
        );
    }

    /// #347: `enter_consent` transitions to `Screen::Consent` carrying
    /// the kind + the selected-ISO index for return-routing.
    #[test]
    fn enter_consent_transitions_to_consent_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_consent(ConsentKind::InstallerCanEraseDisks, 0);
        match s.screen {
            Screen::Consent { kind, selected } => {
                assert_eq!(kind, ConsentKind::InstallerCanEraseDisks);
                assert_eq!(selected, 0);
            }
            other => panic!("expected Screen::Consent, got {other:?}"),
        }
    }

    /// #347: `grant_consent` flips `session_consent` and routes back
    /// to the Confirm screen for the same ISO. Returns `Some(idx)` so
    /// the main-loop handler can chain into kexec dispatch.
    #[test]
    fn grant_consent_sets_session_flag_and_routes_to_confirm() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_consent(ConsentKind::InstallerCanEraseDisks, 0);
        let Some(idx) = s.grant_consent() else {
            panic!("consent grant returns the idx");
        };
        assert_eq!(idx, 0);
        assert!(s.session_consent, "consent must persist for the session");
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
    }

    /// #347: `cancel_consent` returns to Confirm WITHOUT setting the
    /// `session_consent` flag — operator declined.
    #[test]
    fn cancel_consent_does_not_persist_grant() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_consent(ConsentKind::InstallerCanEraseDisks, 0);
        s.cancel_consent();
        assert!(matches!(s.screen, Screen::Confirm { selected: 0 }));
        assert!(!s.session_consent, "cancel must NOT persist consent");
    }

    /// #347: `grant_consent` on a non-Consent screen is a no-op
    /// (returns `None`) so a stray keypress can't toggle
    /// `session_consent` from elsewhere in the state machine.
    #[test]
    fn grant_consent_noop_when_not_on_consent_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        // screen is List by default; calling grant_consent should
        // return None and leave session_consent untouched.
        assert!(s.grant_consent().is_none());
        assert!(!s.session_consent);
    }

    /// #347: the consent flag does NOT bypass the kexec-blocked gate.
    /// Granting consent on a Windows ISO (`NotKexecBootable`) does not
    /// allow kexec to proceed — the security invariant from #602/#558
    /// (`Quirk::NotKexecBootable` always blocks) stays in force.
    #[test]
    fn consent_does_not_bypass_kexec_block() {
        let mut iso = fake_iso("windows.iso");
        iso.quirks = vec![Quirk::NotKexecBootable];
        iso.contains_installer = true;
        let mut s = AppState::new(vec![iso]);
        s.session_consent = true;
        assert!(
            s.is_kexec_blocked(0),
            "NotKexecBootable wins over session_consent"
        );
    }

    /// #602: a successful verify-now after a prior failure clears the
    /// warning — the new JSONL line supersedes the missing one, so the
    /// banner would be stale. `main.rs` calls `clear_audit_warning()`
    /// on the Ok branch of `save_verify_audit_log`.
    #[test]
    fn audit_warning_cleared_after_successful_save() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.set_audit_warning("audit log write failed (transient)");
        // Simulate a subsequent successful save by calling the same
        // method main.rs invokes on the Ok branch.
        s.clear_audit_warning();
        assert!(s.audit_warning.is_none());
    }

    #[test]
    fn is_kexec_blocked_false_for_clean_iso() {
        let s = AppState::new(vec![fake_iso("clean")]);
        assert!(!s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_true_when_quirk_present() {
        let mut iso = fake_iso("windows");
        iso.quirks = vec![Quirk::NotKexecBootable];
        let s = AppState::new(vec![iso]);
        assert!(s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_true_when_hash_mismatch() {
        // A tampered ISO must not be kexec'd even if the operator clicks
        // through the red ✗ MISMATCH warning. Regression for #55.
        let mut iso = fake_iso("tampered");
        iso.hash_verification = HashVerification::Mismatch {
            expected: "abc".to_string(),
            actual: "def".to_string(),
            source: "/run/media/tampered.iso.sha256".to_string(),
        };
        let s = AppState::new(vec![iso]);
        assert!(s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_true_when_signature_forged() {
        // A signature that fails crypto verification under a trusted key
        // must not be kexec'd. Regression for #55.
        let mut iso = fake_iso("forged");
        iso.signature_verification = SignatureVerification::Forged {
            sig_path: std::path::PathBuf::from("/run/media/forged.iso.minisig"),
        };
        let s = AppState::new(vec![iso]);
        assert!(s.is_kexec_blocked(0));
    }

    #[test]
    fn is_kexec_blocked_false_for_unknown_index() {
        let s = AppState::new(vec![fake_iso("a")]);
        assert!(!s.is_kexec_blocked(99));
    }

    #[test]
    fn enter_cmdline_editor_seeds_with_iso_default() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        let Screen::EditCmdline {
            buffer,
            cursor,
            selected,
        } = &s.screen
        else {
            panic!("expected EditCmdline screen");
        };
        assert_eq!(*selected, 0);
        assert_eq!(buffer, "boot=casper"); // from fake_iso
        assert_eq!(*cursor, buffer.len());
    }

    #[test]
    fn cmdline_insert_at_cursor() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_insert(' ');
        s.cmdline_insert('q');
        s.cmdline_insert('u');
        s.cmdline_insert('i');
        s.cmdline_insert('e');
        s.cmdline_insert('t');
        let Screen::EditCmdline { buffer, .. } = &s.screen else {
            panic!("expected EditCmdline")
        };
        assert_eq!(buffer, "boot=casper quiet");
    }

    #[test]
    fn cmdline_backspace_and_cursor_navigation() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        // Buffer is "boot=casper", cursor at end (11).
        s.cmdline_backspace(); // remove 'r'
        s.cmdline_backspace(); // remove 'e'
        let Screen::EditCmdline { buffer, cursor, .. } = &s.screen else {
            panic!()
        };
        assert_eq!(buffer, "boot=casp");
        assert_eq!(*cursor, 9);
        s.cmdline_cursor_left();
        s.cmdline_cursor_left();
        s.cmdline_insert('X');
        let Screen::EditCmdline { buffer, .. } = &s.screen else {
            panic!()
        };
        // cursor=9 after backspaces → left→left moves to 7 (before 's');
        // inserting 'X' there gives "boot=ca" + "X" + "sp".
        assert_eq!(buffer, "boot=caXsp");
    }

    #[test]
    fn cmdline_cursor_home_jumps_to_start() {
        // #544: Home / Ctrl+A should move the cursor to byte 0 regardless
        // of where it is when the key is pressed.
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        // Default buffer is "boot=casper", cursor at end (11).
        s.cmdline_cursor_home();
        let Screen::EditCmdline { cursor, .. } = &s.screen else {
            panic!("expected EditCmdline")
        };
        assert_eq!(*cursor, 0);
        // Inserting 'X' at home should land at index 0.
        s.cmdline_insert('X');
        let Screen::EditCmdline { buffer, cursor, .. } = &s.screen else {
            panic!()
        };
        assert_eq!(buffer, "Xboot=casper");
        assert_eq!(*cursor, 1);
    }

    #[test]
    fn cmdline_cursor_end_jumps_to_buffer_len() {
        // #544: End / Ctrl+E should move the cursor to buffer.len()
        // regardless of where it is when the key is pressed.
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        // Move to the start, then jump to end.
        s.cmdline_cursor_home();
        s.cmdline_cursor_end();
        let Screen::EditCmdline { buffer, cursor, .. } = &s.screen else {
            panic!()
        };
        assert_eq!(*cursor, buffer.len());
        // Append should land after the existing buffer.
        s.cmdline_insert('!');
        let Screen::EditCmdline { buffer, .. } = &s.screen else {
            panic!()
        };
        assert_eq!(buffer, "boot=casper!");
    }

    #[test]
    fn cmdline_cursor_home_and_end_are_idempotent_on_empty_screen() {
        // Defensive: home / end called when not in cmdline editor must
        // not panic (the early-return guard in state.rs covers this).
        let mut s = AppState::new(vec![fake_iso("a")]);
        // Still on List screen — neither key should mutate state or panic.
        s.cmdline_cursor_home();
        s.cmdline_cursor_end();
    }

    #[test]
    fn commit_cmdline_stores_override_and_returns_to_confirm() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_insert(' ');
        s.cmdline_insert('q');
        s.commit_cmdline_edit();
        assert_eq!(s.screen, Screen::Confirm { selected: 0 });
        assert_eq!(
            s.cmdline_overrides.get(&0),
            Some(&"boot=casper q".to_string())
        );
        assert_eq!(s.effective_cmdline(0), "boot=casper q");
    }

    #[test]
    fn cancel_cmdline_edit_preserves_original() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_insert('X');
        s.cancel_cmdline_edit();
        assert_eq!(s.screen, Screen::Confirm { selected: 0 });
        assert!(s.cmdline_overrides.is_empty());
        assert_eq!(s.effective_cmdline(0), "boot=casper"); // unchanged default
    }

    #[test]
    fn backspace_on_empty_buffer_is_noop() {
        let mut s = AppState::new(vec![{
            let mut i = fake_iso("a");
            i.cmdline = None; // no default
            i
        }]);
        s.confirm_selection();
        s.enter_cmdline_editor();
        s.cmdline_backspace();
        s.cmdline_cursor_left();
        let Screen::EditCmdline { buffer, cursor, .. } = &s.screen else {
            panic!()
        };
        assert_eq!(buffer, "");
        assert_eq!(*cursor, 0);
    }

    #[test]
    fn effective_cmdline_prefers_override() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.effective_cmdline(0), "boot=casper");
        s.cmdline_overrides.insert(0, "override=yes".to_string());
        assert_eq!(s.effective_cmdline(0), "override=yes");
    }

    #[test]
    fn quirks_summary_empty_for_clean_iso() {
        let iso = fake_iso("clean");
        assert_eq!(quirks_summary(&iso), "");
    }

    #[test]
    fn quirks_summary_lists_each_quirk() {
        let mut iso = fake_iso("warn");
        iso.quirks = vec![Quirk::UnsignedKernel, Quirk::BiosOnly];
        let s = quirks_summary(&iso);
        assert!(s.contains("unsigned-kernel"));
        assert!(s.contains("bios-only"));
    }

    // ---- #458 — dual-pane focus + info-scroll -----------------------

    #[test]
    fn default_pane_is_list() {
        let s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.pane, Pane::List);
    }

    #[test]
    fn toggle_pane_swaps_focus_on_list_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.pane, Pane::List);
        s.toggle_pane();
        assert_eq!(s.pane, Pane::Info);
        s.toggle_pane();
        assert_eq!(s.pane, Pane::List);
    }

    #[test]
    fn toggle_pane_noop_outside_list_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.screen = Screen::Confirm { selected: 0 };
        s.toggle_pane();
        assert_eq!(
            s.pane,
            Pane::List,
            "pane must not change when not on List screen"
        );
    }

    #[test]
    fn move_info_scroll_increments_and_saturates_at_zero() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        assert_eq!(s.info_scroll, 0);
        s.move_info_scroll(3);
        assert_eq!(s.info_scroll, 3);
        s.move_info_scroll(-5);
        assert_eq!(s.info_scroll, 0, "saturates at zero");
    }

    #[test]
    fn move_info_scroll_does_not_change_pane_focus() {
        // #545: Shift+↑ / Shift+↓ on List should scroll info pane WITHOUT
        // moving focus off the list. The keybinding (in main.rs) calls
        // `move_info_scroll` directly; this test pins the contract that
        // the function does NOT mutate `pane`. If a future refactor adds
        // pane-toggle side effects here, this test catches the regression
        // and forces the keybinding to be reconsidered.
        let mut s = AppState::new(vec![fake_iso("a")]);
        let pane_before = s.pane;
        s.move_info_scroll(3);
        s.move_info_scroll(-1);
        assert_eq!(
            s.pane, pane_before,
            "info-scroll must not mutate pane focus (Shift+arrow contract)"
        );
    }

    #[test]
    fn move_selection_resets_info_scroll() {
        // Per-ISO scroll state would confuse operators; resetting on
        // selection change matches gitui's pattern.
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        s.info_scroll = 7;
        s.move_selection(1);
        assert_eq!(s.info_scroll, 0);
    }

    #[test]
    fn move_selection_noop_does_not_reset_info_scroll() {
        // When the cursor hits a saturation boundary, info_scroll
        // should not be reset — it wasn't a real navigation.
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.info_scroll = 5;
        s.move_selection(-1); // already at 0, can't go further up
        assert_eq!(s.info_scroll, 5);
    }

    #[test]
    fn pane_toggle_method_returns_opposite() {
        assert_eq!(Pane::List.toggle(), Pane::Info);
        assert_eq!(Pane::Info.toggle(), Pane::List);
    }

    // ---- Delete-with-confirmation transitions ---------------------

    #[test]
    fn enter_delete_transitions_to_confirm_delete_for_real_iso() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        let Some(path) = s.enter_delete(0) else {
            panic!("real ISO row should accept delete");
        };
        assert!(path.to_string_lossy().contains('a'));
        match s.screen {
            Screen::ConfirmDelete { selected } => assert_eq!(selected, 0),
            other => panic!("expected ConfirmDelete, got {other:?}"),
        }
    }

    #[test]
    fn enter_delete_returns_none_outside_list_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.screen = Screen::Confirm { selected: 0 };
        assert!(
            s.enter_delete(0).is_none(),
            "delete prompt must not open from non-List screen"
        );
        assert!(
            matches!(s.screen, Screen::Confirm { .. }),
            "screen must be unchanged on refusal"
        );
    }

    #[test]
    fn enter_delete_returns_none_for_rescue_shell_row() {
        // The rescue-shell synthetic row is the last entry. Cursor at
        // that index must NOT open the delete prompt — there's no
        // on-disk file backing it.
        let mut s = AppState::new(vec![fake_iso("a")]);
        let last = s.visible_entries().len() - 1; // == 1 (one ISO + shell)
        assert!(
            s.enter_delete(last).is_none(),
            "rescue-shell row must not be deletable"
        );
        assert!(
            matches!(s.screen, Screen::List { .. }),
            "screen unchanged when cursor is on a non-deletable row"
        );
    }

    #[test]
    fn cancel_delete_returns_to_list_preserving_cursor() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b")]);
        let _ = s.enter_delete(1);
        s.cancel_delete();
        match s.screen {
            Screen::List { selected } => assert_eq!(selected, 1, "cursor preserved on cancel"),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn delete_completed_removes_iso_and_clamps_cursor() {
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        // Sort defaults to Name, so visible order is a, b, c, <shell>.
        let _ = s.enter_delete(2); // pick "c"
        s.delete_completed();
        assert_eq!(s.isos.len(), 2, "isos vec shrunk");
        assert!(
            s.isos.iter().all(|i| !i.label.contains('c')),
            "deleted ISO removed from vec"
        );
        match s.screen {
            Screen::List { selected } => {
                // Visible len now = 2 ISOs + shell = 3; cursor was 2,
                // new last is 2, so it stays put.
                assert_eq!(selected, 2);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn delete_completed_clamps_cursor_when_last_iso_removed() {
        let mut s = AppState::new(vec![fake_iso("only")]);
        let _ = s.enter_delete(0);
        s.delete_completed();
        // Now list = [<rescue-shell>] only — the synthetic row.
        match s.screen {
            Screen::List { selected } => {
                assert_eq!(
                    selected, 0,
                    "cursor clamped to 0 when only the rescue-shell row remains"
                );
            }
            other => panic!("expected List, got {other:?}"),
        }
        let entries = s.visible_entries();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0], ViewEntry::RescueShell));
    }

    #[test]
    fn delete_completed_shifts_cmdline_overrides_for_higher_indices() {
        // Operator edited cmdline of ISO at idx=2, then deletes ISO at
        // idx=1. The override must follow the surviving ISO down to
        // its new position (idx=1), not stay attached to a now-empty
        // slot or float into a different ISO.
        let mut s = AppState::new(vec![fake_iso("a"), fake_iso("b"), fake_iso("c")]);
        s.cmdline_overrides.insert(2, "console=ttyS0".to_string());
        // sort=Name → visible order matches insertion. enter_delete(1)
        // picks "b" (real_idx=1 too).
        let _ = s.enter_delete(1);
        s.delete_completed();
        assert_eq!(
            s.cmdline_overrides.get(&1),
            Some(&"console=ttyS0".to_string()),
            "override at idx=2 must shift down to idx=1 after the deletion"
        );
        assert!(
            !s.cmdline_overrides.contains_key(&2),
            "no stale override at the now-out-of-range idx=2"
        );
    }

    #[test]
    fn record_delete_error_transitions_to_error_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        let _ = s.enter_delete(0);
        s.record_delete_error("read-only filesystem");
        match s.screen {
            Screen::Error {
                message,
                remedy,
                return_to,
            } => {
                assert!(message.contains("read-only filesystem"));
                assert_eq!(return_to, 0);
                assert!(remedy.is_some(), "delete-error always carries remedy");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // ---- Network overlay (#655 Phase 1B) -------------------------

    fn fake_iface(name: &str, up: bool) -> crate::network::NetworkIface {
        crate::network::NetworkIface {
            name: name.to_string(),
            link_state: if up {
                crate::network::LinkState::Up
            } else {
                crate::network::LinkState::Down
            },
            ipv4: None,
        }
    }

    #[test]
    fn enter_network_stores_prior_screen_for_dismiss() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        // Start on List with cursor at 1.
        s.screen = Screen::List { selected: 1 };
        s.enter_network(vec![fake_iface("eth0", true)]);
        match &s.screen {
            Screen::Network {
                interfaces,
                selected,
                op,
                prior,
            } => {
                assert_eq!(interfaces.len(), 1);
                assert_eq!(*selected, 0);
                assert_eq!(*op, NetworkOp::Idle);
                assert!(matches!(**prior, Screen::List { selected: 1 }));
            }
            other => panic!("expected Network, got {other:?}"),
        }
    }

    #[test]
    fn cancel_network_restores_prior_screen() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.screen = Screen::List { selected: 0 };
        s.enter_network(vec![fake_iface("eth0", true)]);
        s.cancel_network();
        assert!(matches!(s.screen, Screen::List { selected: 0 }));
    }

    #[test]
    fn enter_network_is_noop_when_already_in_network() {
        // Re-entering Network from Network must not reset the cursor /
        // op state — cheap accidental keypress shouldn't lose context.
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true), fake_iface("eth1", false)]);
        s.network_move_selection(1);
        s.enter_network(vec![fake_iface("foo", true)]);
        match &s.screen {
            Screen::Network {
                interfaces,
                selected,
                ..
            } => {
                assert_eq!(interfaces.len(), 2, "second enter_network ignored");
                assert_eq!(*selected, 1, "cursor preserved");
            }
            other => panic!("expected Network, got {other:?}"),
        }
    }

    #[test]
    fn network_move_selection_clamps_within_bounds() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true), fake_iface("eth1", true)]);
        s.network_move_selection(-5); // saturate at 0
        assert!(
            matches!(s.screen, Screen::Network { selected: 0, .. }),
            "underflow clamped to 0"
        );
        s.network_move_selection(99); // saturate at len-1
        assert!(
            matches!(s.screen, Screen::Network { selected: 1, .. }),
            "overflow clamped to last index"
        );
    }

    #[test]
    fn network_begin_dhcp_returns_iface_and_flips_to_pending() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true)]);
        let Some(iface) = s.network_begin_dhcp() else {
            panic!("Idle → Pending should return iface name");
        };
        assert_eq!(iface, "eth0");
        match &s.screen {
            Screen::Network {
                op: NetworkOp::Pending { iface, last_status },
                ..
            } => {
                assert_eq!(iface, "eth0");
                assert!(last_status.is_empty());
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn network_begin_dhcp_is_noop_during_pending() {
        // Double-Enter while a worker is still running must not
        // double-spawn — Pending → Pending suppression.
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true)]);
        let _ = s.network_begin_dhcp();
        assert!(
            s.network_begin_dhcp().is_none(),
            "second begin_dhcp during Pending must return None"
        );
    }

    #[test]
    fn network_finish_dhcp_success_updates_row_ipv4_and_op() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true)]);
        let _ = s.network_begin_dhcp();
        let lease = crate::network::NetworkLease {
            ipv4: "192.168.1.42/24".to_string(),
            gateway: Some("192.168.1.1".to_string()),
            nameservers: vec!["8.8.8.8".to_string()],
        };
        s.network_finish_dhcp("eth0".to_string(), Ok(lease.clone()));
        match &s.screen {
            Screen::Network {
                interfaces,
                op: NetworkOp::Success { iface, lease: got },
                ..
            } => {
                assert_eq!(interfaces[0].ipv4.as_deref(), Some("192.168.1.42/24"));
                assert_eq!(iface, "eth0");
                assert_eq!(got, &lease);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[test]
    fn network_finish_dhcp_failure_keeps_row_ipv4_unchanged() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true)]);
        let _ = s.network_begin_dhcp();
        s.network_finish_dhcp("eth0".to_string(), Err("NAK from server".to_string()));
        match &s.screen {
            Screen::Network {
                interfaces,
                op: NetworkOp::Failed { iface, err },
                ..
            } => {
                assert_eq!(interfaces[0].ipv4, None, "iface row unchanged on failure");
                assert_eq!(iface, "eth0");
                assert!(err.contains("NAK"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn network_progress_updates_pending_status_only() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true)]);
        // Progress before any begin_dhcp should be a no-op (we're still
        // in Idle).
        s.network_progress("ignored".to_string());
        assert!(
            matches!(
                s.screen,
                Screen::Network {
                    op: NetworkOp::Idle,
                    ..
                }
            ),
            "progress in Idle must not advance state"
        );
        let _ = s.network_begin_dhcp();
        s.network_progress("trying lease 2/5".to_string());
        match &s.screen {
            Screen::Network {
                op: NetworkOp::Pending { last_status, .. },
                ..
            } => {
                assert_eq!(last_status, "trying lease 2/5");
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn network_refresh_replaces_iface_table_and_resets_op() {
        let mut s = AppState::new(vec![fake_iso("a")]);
        s.enter_network(vec![fake_iface("eth0", true)]);
        let _ = s.network_begin_dhcp();
        s.network_refresh(vec![
            fake_iface("eth1", true),
            fake_iface("eth2", true),
            fake_iface("eth3", false),
        ]);
        match &s.screen {
            Screen::Network {
                interfaces,
                selected,
                op,
                ..
            } => {
                assert_eq!(interfaces.len(), 3);
                assert_eq!(*selected, 0, "cursor reset on refresh");
                assert_eq!(*op, NetworkOp::Idle, "op reset on refresh");
            }
            other => panic!("expected Network, got {other:?}"),
        }
    }
}
