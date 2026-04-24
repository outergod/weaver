//! TUI render loop — crossterm raw-mode event loop that multiplexes
//! keystrokes, inbound bus messages, and disconnect detection.
//!
//! Implements fact rendering, stale-on-disconnect, and the display
//! side of `[i]nspect` (wires through [`crate::commands`]). Slice 003
//! retired the `e`/`c` simulate-edit/simulate-clean keystrokes when
//! `buffer/dirty` authority moved to the `weaver-buffers` service;
//! in-TUI editing arrives in slice 004.
//!
//! Render surface matches `specs/003-buffer-service/contracts/cli-surfaces.md`:
//!
//! ```text
//! ┌─ Weaver TUI ────────────────────────────────────────────────────┐
//! │ Connection: ready (bus v0.3.0)                                  │
//! │                                                                 │
//! │ Facts:                                                          │
//! │   buffer/dirty(EntityRef(1)) = true                             │
//! │     by service weaver-buffers (inst …), event N, 0.142s ago    │
//! │                                                                 │
//! │ Commands: [i]nspect  [q]uit                                     │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use crossterm::{cursor, execute, queue, style::Print, terminal};
use miette::{IntoDiagnostic, miette};
use tokio::sync::mpsc;

use weaver_core::provenance::ActorIdentity;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{
    BUS_PROTOCOL_VERSION_STR, BusMessage, InspectionDetail, InspectionError,
};

use crate::client::{BusStreamItem, TuiClient, connect};
use crate::commands;

/// What the render layer knows about a currently-asserted fact.
struct FactDisplay {
    fact: Fact,
    asserted_at_wall_ns: u64,
}

enum ConnStatus {
    Ready,
    Unavailable { reason: String },
}

/// The latest inspection outcome the user has requested, with its
/// target key for rendering.
struct InspectionView {
    fact: FactKey,
    result: Result<InspectionDetail, InspectionError>,
}

struct AppState {
    facts: HashMap<FactKey, FactDisplay>,
    status: ConnStatus,
    stale: bool,
    /// `Some((request_id, fact_key))` when an `InspectRequest` is in
    /// flight; cleared on matching `InspectResponse`.
    pending_inspection: Option<(u64, FactKey)>,
    /// Last completed inspection, for rendering beneath the facts.
    last_inspection: Option<InspectionView>,
}

impl AppState {
    fn new() -> Self {
        Self {
            facts: HashMap::new(),
            status: ConnStatus::Ready,
            stale: false,
            pending_inspection: None,
            last_inspection: None,
        }
    }

    fn is_available(&self) -> bool {
        matches!(self.status, ConnStatus::Ready)
    }

    fn apply(&mut self, msg: BusMessage) {
        match msg {
            BusMessage::FactAssert(fact) => {
                self.facts.insert(
                    fact.key.clone(),
                    FactDisplay {
                        fact,
                        asserted_at_wall_ns: wall_ns(),
                    },
                );
            }
            BusMessage::FactRetract { key, .. } => {
                self.facts.remove(&key);
            }
            BusMessage::InspectResponse { request_id, result } => {
                if let Some((pending_id, pending_key)) = self.pending_inspection.take() {
                    if pending_id == request_id {
                        self.last_inspection = Some(InspectionView {
                            fact: pending_key,
                            result,
                        });
                    } else {
                        // Not our response — restore the pending slot
                        // and keep waiting.
                        self.pending_inspection = Some((pending_id, pending_key));
                    }
                }
            }
            // Lifecycle / Error / other server-originated messages are
            // informational only in slice 001; the render layer does
            // not surface them yet.
            _ => {}
        }
    }

    fn mark_unavailable(&mut self, reason: String) {
        self.status = ConnStatus::Unavailable { reason };
        self.stale = true;
    }
}

/// Entry point invoked from `lib::run`.
///
/// Order-independent startup per `cli-surfaces.md` — if the core isn't
/// running yet, the TUI still enters raw mode and renders the
/// `UNAVAILABLE` state. The user can hit `[r]econnect` once they
/// start the core, or `[q]uit` to exit.
pub async fn run(socket: PathBuf) -> miette::Result<()> {
    // Initial connection attempt. Failure is a documented state, not
    // a fatal error — fall through to raw-mode with `UNAVAILABLE`.
    let mut client: Option<TuiClient> = connect(&socket).await.ok();
    let mut state = AppState::new();
    if client.is_none() {
        state.mark_unavailable(format!(
            "core not reachable at {} (press `r` to retry)",
            socket.display()
        ));
    }

    // Terminal setup.
    terminal::enable_raw_mode().into_diagnostic()?;
    let mut out = stdout();
    execute!(
        out,
        terminal::EnterAlternateScreen,
        cursor::Hide,
        terminal::Clear(terminal::ClearType::All),
    )
    .into_diagnostic()?;
    let _guard = RawModeGuard;

    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    let key_task = spawn_key_reader(key_tx);

    draw(&mut out, &state).map_err(|e| miette!("draw: {e}"))?;

    let result: miette::Result<()> = loop {
        // `tokio::select!` uses the `if` guard to avoid polling
        // `client.inbound.recv()` when we're not connected. When
        // unavailable, only keystrokes wake the loop (which is fine —
        // the user must press `r` or `q`).
        tokio::select! {
            maybe_key = key_rx.recv() => {
                match maybe_key {
                    Some(key) => {
                        if should_quit(&key) {
                            break Ok(());
                        }
                        if state.is_available() {
                            if let Some(c) = client.as_mut() {
                                handle_key_when_ready(&key, &mut state, c).await;
                            }
                        } else if is_reconnect_key(&key) {
                            match connect(&socket).await {
                                Ok(new_client) => {
                                    // Abort the old reader if any (should be
                                    // gone already, but be defensive).
                                    if let Some(old) = client.take() {
                                        old.reader_task.abort();
                                    }
                                    client = Some(new_client);
                                    // Reset view state — the fresh
                                    // subscription replays the current
                                    // snapshot, so we shouldn't show stale
                                    // facts while waiting for it.
                                    state = AppState::new();
                                }
                                Err(e) => {
                                    state.mark_unavailable(format!(
                                        "reconnect failed: {e} (press `r` to retry)"
                                    ));
                                }
                            }
                        }
                    }
                    None => break Ok(()),
                }
            }
            maybe_msg = async {
                client.as_mut().unwrap().inbound.recv().await
            }, if client.is_some() => {
                match maybe_msg {
                    Some(item) => apply_inbound(&mut state, item),
                    None => {
                        // Reader task ended — mark the connection dead.
                        if let Some(old) = client.take() {
                            old.reader_task.abort();
                        }
                        state.mark_unavailable(
                            "bus reader ended (press `r` to reconnect)".into(),
                        );
                    }
                }
            }
        }
        if let Err(e) = draw(&mut out, &state) {
            break Err(miette!("draw: {e}"));
        }
    };

    // Cleanup.
    key_task.abort();
    if let Some(c) = client.as_ref() {
        c.reader_task.abort();
    }
    let _ = execute!(
        out,
        cursor::Show,
        terminal::LeaveAlternateScreen,
        terminal::Clear(terminal::ClearType::All),
    );

    result
}

fn apply_inbound(state: &mut AppState, item: BusStreamItem) {
    match item {
        Ok(msg) => state.apply(msg),
        Err(e) => state.mark_unavailable(format!("bus stream error: {e}")),
    }
}

fn should_quit(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc
    ) || (key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
}

fn is_inspect_key(key: &KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    matches!(key.code, KeyCode::Char('i') | KeyCode::Char('I'))
}

fn is_reconnect_key(key: &KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
}

/// Handle a keystroke while connected. Factored out so the main loop
/// stays readable.
async fn handle_key_when_ready(key: &KeyEvent, state: &mut AppState, client: &mut TuiClient) {
    if is_inspect_key(key) {
        // F29 review fix: `[i]` must inspect the fact that is
        // visually first in the rendered output. Prior logic used
        // `state.facts.keys().next()` — HashMap iteration order —
        // so after the Facts / Repositories split the inspected
        // fact could be anywhere on screen (or in the Repositories
        // section) while the operator looked at something else.
        if let Some(target_key) = inspect_target(state) {
            match commands::inspect(&mut client.writer, target_key.clone()).await {
                Ok(request_id) => {
                    state.pending_inspection = Some((request_id, target_key));
                }
                Err(e) => {
                    state.mark_unavailable(format!("write failed: {e} (press `r` to reconnect)"));
                }
            }
        }
    }
}

/// Spawn a blocking thread to poll for terminal events. Crossterm's
/// blocking `read` integrates cleanly with `spawn_blocking`; this
/// avoids an additional `futures_util::StreamExt` dependency.
fn spawn_key_reader(tx: mpsc::UnboundedSender<KeyEvent>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        loop {
            // 100ms poll keeps the thread responsive to abort/shutdown.
            match crossterm::event::poll(std::time::Duration::from_millis(100)) {
                Ok(true) => match crossterm::event::read() {
                    Ok(CtEvent::Key(k)) => {
                        if tx.send(k).is_err() {
                            return;
                        }
                    }
                    Ok(_) => continue,
                    Err(_) => return,
                },
                Ok(false) => continue,
                Err(_) => return,
            }
        }
    })
}

fn draw<W: Write>(w: &mut W, state: &AppState) -> std::io::Result<()> {
    execute!(
        w,
        cursor::MoveTo(0, 0),
        terminal::Clear(terminal::ClearType::All),
    )?;

    let mut row: u16 = 0;
    let emit = |w: &mut W, row: &mut u16, line: String| -> std::io::Result<()> {
        queue!(w, cursor::MoveTo(0, *row), Print(line))?;
        *row += 1;
        Ok(())
    };

    emit(
        w,
        &mut row,
        "┌─ Weaver TUI ────────────────────────────────────────────────────┐".into(),
    )?;
    match &state.status {
        ConnStatus::Ready => {
            emit(
                w,
                &mut row,
                format!("│ Connection: ready (bus v{BUS_PROTOCOL_VERSION_STR})"),
            )?;
        }
        ConnStatus::Unavailable { reason } => {
            emit(w, &mut row, "│ Connection: UNAVAILABLE".into())?;
            emit(w, &mut row, format!("│   reason: {reason}"))?;
            if !state.facts.is_empty() {
                emit(
                    w,
                    &mut row,
                    "│   facts shown below are the last-known view (may be stale)".into(),
                )?;
            }
        }
    }
    emit(w, &mut row, "│".into())?;

    let label = if state.stale {
        "Facts (stale):"
    } else {
        "Facts:"
    };
    emit(w, &mut row, format!("│ {label}"))?;
    // Only non-repo facts land here; repositories get their own
    // section below per contracts/cli-surfaces.md §Repositories.
    // Ordering is deterministic so the Facts section doesn't
    // shuffle between ticks and `[i]` always targets the visually-
    // first fact (F29 review fix).
    let non_repo: Vec<&FactDisplay> = non_repo_fact_keys_sorted(state)
        .iter()
        .filter_map(|k| state.facts.get(k))
        .collect();
    if non_repo.is_empty() {
        emit(w, &mut row, "│   (none)".into())?;
    } else {
        for fd in &non_repo {
            emit(
                w,
                &mut row,
                format!(
                    "│   {}({}) = {}",
                    fd.fact.key.attribute,
                    fd.fact.key.entity,
                    format_value(&fd.fact.value),
                ),
            )?;
            emit(
                w,
                &mut row,
                format!(
                    "│     {}",
                    annotation(&fd.fact, fd.asserted_at_wall_ns, state.stale)
                ),
            )?;
        }
    }
    emit(w, &mut row, "│".into())?;

    let repo_label = if state.stale {
        "Repositories (stale):"
    } else {
        "Repositories:"
    };
    emit(w, &mut row, format!("│ {repo_label}"))?;
    let repo_views = collect_repo_views(&state.facts);
    if repo_views.is_empty() {
        emit(w, &mut row, "│   (none)".into())?;
    } else {
        for view in &repo_views {
            let path = view.path.unwrap_or("(path unknown)");
            let badge = format_state_badge(view.state.as_ref());
            let dirty_or_obs = format_dirty_or_obs_lost(view.observable, view.dirty);
            let stale_tail = if state.stale { " [stale]" } else { "" };
            emit(
                w,
                &mut row,
                format!("│   {path}  {badge} {dirty_or_obs}{stale_tail}"),
            )?;
            if let Some(sha) = view.head_commit {
                emit(w, &mut row, format!("│     head: {}", short_sha(sha)))?;
            }
            if let Some((fact, asserted_at)) = view.representative {
                emit(
                    w,
                    &mut row,
                    format!("│     {}", annotation(fact, asserted_at, state.stale)),
                )?;
            }
        }
    }
    emit(w, &mut row, "│".into())?;

    // Buffers section — rendered beneath Repositories per spec FR-014.
    // One row per buffer entity: `<path> [<bytes> bytes] <dirty-badge>`
    // plus the shared `by service …` annotation. The dirty badge is
    // replaced by `[observability lost]` when `buffer/observable=false`,
    // and every row gets a `[stale]` tail when the TUI has lost its
    // core subscription (per `contracts/cli-surfaces.md §Display rules`).
    let buffer_label = if state.stale {
        "Buffers (stale):"
    } else {
        "Buffers:"
    };
    emit(w, &mut row, format!("│ {buffer_label}"))?;
    let buffer_views = collect_buffer_views(&state.facts);
    if buffer_views.is_empty() {
        emit(w, &mut row, "│   (none)".into())?;
    } else {
        for view in &buffer_views {
            let path = view.path.unwrap_or("(path unknown)");
            let size = format_byte_size(view.byte_size);
            let dirty_or_obs = format_dirty_or_obs_lost(view.observable, view.dirty);
            let stale_tail = if state.stale { " [stale]" } else { "" };
            emit(
                w,
                &mut row,
                format!("│   {path}  {size} {dirty_or_obs}{stale_tail}"),
            )?;
            if let Some((fact, asserted_at)) = view.representative {
                emit(
                    w,
                    &mut row,
                    format!("│     {}", annotation(fact, asserted_at, state.stale)),
                )?;
            }
        }
    }
    emit(w, &mut row, "│".into())?;

    if let Some(view) = &state.last_inspection {
        emit(w, &mut row, "│ Inspection:".into())?;
        emit(
            w,
            &mut row,
            format!("│   fact: {}({})", view.fact.attribute, view.fact.entity),
        )?;
        match &view.result {
            Ok(detail) => {
                emit(
                    w,
                    &mut row,
                    format!("│   source_event:       {}", detail.source_event),
                )?;
                if let Some(b) = &detail.asserting_behavior {
                    emit(w, &mut row, format!("│   asserting_behavior: {b}"))?;
                }
                if let Some(svc) = &detail.asserting_service {
                    emit(w, &mut row, format!("│   asserting_service:  {svc}"))?;
                }
                if let Some(inst) = &detail.asserting_instance {
                    emit(w, &mut row, format!("│   asserting_instance: {inst}"))?;
                }
                emit(
                    w,
                    &mut row,
                    format!("│   asserted_at_ns:     {}", detail.asserted_at_ns),
                )?;
                emit(
                    w,
                    &mut row,
                    format!("│   trace_sequence:     {}", detail.trace_sequence),
                )?;
            }
            Err(e) => {
                emit(w, &mut row, format!("│   error: {e:?}"))?;
            }
        }
        emit(w, &mut row, "│".into())?;
    } else if state.pending_inspection.is_some() {
        emit(w, &mut row, "│ Inspection: (waiting for response…)".into())?;
        emit(w, &mut row, "│".into())?;
    }

    match &state.status {
        ConnStatus::Ready => emit(w, &mut row, "│ Commands: [i]nspect  [q]uit".into())?,
        ConnStatus::Unavailable { .. } => {
            emit(w, &mut row, "│ Commands: [r]econnect  [q]uit".into())?
        }
    }
    emit(
        w,
        &mut row,
        "└─────────────────────────────────────────────────────────────────┘".into(),
    )?;

    w.flush()?;
    Ok(())
}

/// Render order for catch-all fact keys: sort by `(entity, attribute)`
/// so the Facts section is deterministic across ticks (F29 review
/// fix — `HashMap` iteration would otherwise shuffle the rendered
/// order between runs and the `[i]` target wouldn't match what the
/// operator sees first).
///
/// Filters out `repo/*` and `buffer/*` — those have their own
/// Repositories and Buffers sections and would double-render here.
fn non_repo_fact_keys_sorted(state: &AppState) -> Vec<FactKey> {
    let mut keys: Vec<FactKey> = state
        .facts
        .keys()
        .filter(|k| {
            !is_repo_fact_attribute(&k.attribute) && !is_buffer_fact_attribute(&k.attribute)
        })
        .cloned()
        .collect();
    keys.sort_by(|a, b| {
        a.entity
            .as_u64()
            .cmp(&b.entity.as_u64())
            .then_with(|| a.attribute.cmp(&b.attribute))
    });
    keys
}

/// The fact key that `[i]nspect` should target: the first rendered
/// fact in visual order. Render order is Facts → Repositories →
/// Buffers, so `[i]` targets the first catch-all fact, falls through
/// to the first repository's representative, then to the first
/// buffer's representative.
fn inspect_target(state: &AppState) -> Option<FactKey> {
    if let Some(k) = non_repo_fact_keys_sorted(state).into_iter().next() {
        return Some(k);
    }
    if let Some(k) = collect_repo_views(&state.facts)
        .into_iter()
        .next()
        .and_then(|v| v.representative.map(|(f, _)| f.key.clone()))
    {
        return Some(k);
    }
    collect_buffer_views(&state.facts)
        .into_iter()
        .next()
        .and_then(|v| v.representative.map(|(f, _)| f.key.clone()))
}

/// Does `attribute` belong to the `repo/*` family? Used to split the
/// Facts section from the Repositories section in the TUI render.
/// Matches what the watcher publishes: `repo/path`,
/// `repo/head-commit`, `repo/dirty`, `repo/observable`, and
/// `repo/state/*`.
fn is_repo_fact_attribute(attribute: &str) -> bool {
    attribute == "repo" || attribute.starts_with("repo/")
}

/// Does `attribute` belong to the `buffer/*` family? Used to split
/// the Facts section from the Buffers section. Matches what
/// `weaver-buffers` publishes: `buffer/path`, `buffer/byte-size`,
/// `buffer/dirty`, `buffer/observable`.
fn is_buffer_fact_attribute(attribute: &str) -> bool {
    attribute == "buffer" || attribute.starts_with("buffer/")
}

/// One repository's collapsed view, assembled from the currently-
/// asserted `repo/*` facts for a single `EntityRef`.
struct RepoView<'a> {
    path: Option<&'a str>,
    state: Option<RepoStateBadge<'a>>,
    /// `Some(true)` when `repo/observable` is asserted as `true`;
    /// `Some(false)` when asserted as `false`; `None` if the fact is
    /// absent.
    observable: Option<bool>,
    head_commit: Option<&'a str>,
    dirty: Option<bool>,
    /// Any `repo/*` fact from this entity — used to pull the
    /// service-identity `by` line via the shared `annotation`
    /// helper. All repo facts for one entity share the same
    /// asserting identity under F14, so any one will do.
    representative: Option<(&'a Fact, u64)>,
}

enum RepoStateBadge<'a> {
    OnBranch(&'a str),
    Detached(&'a str),
    Unborn(&'a str),
}

/// Group `repo/*` facts by `EntityRef` and assemble a `RepoView` per
/// repository. Ordering is deterministic (`BTreeMap` on entity id) so
/// the TUI doesn't flicker between ticks.
fn collect_repo_views(facts: &HashMap<FactKey, FactDisplay>) -> Vec<RepoView<'_>> {
    let mut grouped: BTreeMap<EntityRef, RepoView<'_>> = BTreeMap::new();
    for fd in facts.values() {
        if !is_repo_fact_attribute(&fd.fact.key.attribute) {
            continue;
        }
        let entry = grouped.entry(fd.fact.key.entity).or_insert(RepoView {
            path: None,
            state: None,
            observable: None,
            head_commit: None,
            dirty: None,
            representative: None,
        });
        match fd.fact.key.attribute.as_str() {
            "repo/path" => {
                if let FactValue::String(s) = &fd.fact.value {
                    entry.path = Some(s.as_str());
                }
            }
            "repo/state/on-branch" => {
                if let FactValue::String(s) = &fd.fact.value {
                    entry.state = Some(RepoStateBadge::OnBranch(s.as_str()));
                }
            }
            "repo/state/detached" => {
                if let FactValue::String(s) = &fd.fact.value {
                    entry.state = Some(RepoStateBadge::Detached(s.as_str()));
                }
            }
            "repo/state/unborn" => {
                if let FactValue::String(s) = &fd.fact.value {
                    entry.state = Some(RepoStateBadge::Unborn(s.as_str()));
                }
            }
            "repo/observable" => {
                if let FactValue::Bool(b) = fd.fact.value {
                    entry.observable = Some(b);
                }
            }
            "repo/head-commit" => {
                if let FactValue::String(s) = &fd.fact.value {
                    entry.head_commit = Some(s.as_str());
                }
            }
            "repo/dirty" => {
                if let FactValue::Bool(b) = fd.fact.value {
                    entry.dirty = Some(b);
                }
            }
            _ => {}
        }
        // F27 review fix: use the freshest repo/* fact as the
        // representative for the annotation line. Prior revision
        // took the first fact encountered in HashMap iteration
        // order, which could be `repo/path` — asserted once at
        // startup and never refreshed — while
        // `repo/state/*`/`repo/dirty` tick every poll. Operators
        // watching the elapsed age as a freshness signal would
        // see a frozen clock even though the watcher was healthy.
        match entry.representative {
            None => entry.representative = Some((&fd.fact, fd.asserted_at_wall_ns)),
            Some((_, prev_ns)) if fd.asserted_at_wall_ns > prev_ns => {
                entry.representative = Some((&fd.fact, fd.asserted_at_wall_ns));
            }
            _ => {}
        }
    }
    grouped.into_values().collect()
}

/// One buffer's collapsed view, assembled from the currently-asserted
/// `buffer/*` facts for a single `EntityRef`.
struct BufferView<'a> {
    path: Option<&'a str>,
    byte_size: Option<u64>,
    dirty: Option<bool>,
    /// `Some(true)` when `buffer/observable` is asserted as `true`;
    /// `Some(false)` when asserted as `false`; `None` if the fact is
    /// absent. Drives the `[observability lost]` badge, which
    /// suppresses the dirty flag when the buffer is unobservable
    /// (per `contracts/cli-surfaces.md §Display rules`).
    observable: Option<bool>,
    /// Freshest `buffer/*` fact for this entity — used to pull the
    /// `by service …` line via the shared `annotation` helper. All
    /// buffer facts for one entity share the same asserting identity
    /// (slice-002 F14), so any one will do; using the freshest keeps
    /// the displayed age moving per poll tick (mirrors F27).
    representative: Option<(&'a Fact, u64)>,
}

/// Group `buffer/*` facts by `EntityRef` and assemble a `BufferView`
/// per buffer. Deterministic ordering via `BTreeMap<EntityRef, _>` so
/// the Buffers section doesn't flicker between ticks.
fn collect_buffer_views(facts: &HashMap<FactKey, FactDisplay>) -> Vec<BufferView<'_>> {
    let mut grouped: BTreeMap<EntityRef, BufferView<'_>> = BTreeMap::new();
    for fd in facts.values() {
        if !is_buffer_fact_attribute(&fd.fact.key.attribute) {
            continue;
        }
        let entry = grouped.entry(fd.fact.key.entity).or_insert(BufferView {
            path: None,
            byte_size: None,
            dirty: None,
            observable: None,
            representative: None,
        });
        match fd.fact.key.attribute.as_str() {
            "buffer/path" => {
                if let FactValue::String(s) = &fd.fact.value {
                    entry.path = Some(s.as_str());
                }
            }
            "buffer/byte-size" => {
                if let FactValue::U64(n) = fd.fact.value {
                    entry.byte_size = Some(n);
                }
            }
            "buffer/dirty" => {
                if let FactValue::Bool(b) = fd.fact.value {
                    entry.dirty = Some(b);
                }
            }
            "buffer/observable" => {
                if let FactValue::Bool(b) = fd.fact.value {
                    entry.observable = Some(b);
                }
            }
            _ => {}
        }
        match entry.representative {
            None => entry.representative = Some((&fd.fact, fd.asserted_at_wall_ns)),
            Some((_, prev_ns)) if fd.asserted_at_wall_ns > prev_ns => {
                entry.representative = Some((&fd.fact, fd.asserted_at_wall_ns));
            }
            _ => {}
        }
    }
    grouped.into_values().collect()
}

fn format_state_badge(state: Option<&RepoStateBadge<'_>>) -> String {
    match state {
        Some(RepoStateBadge::OnBranch(name)) => format!("[on {name}]"),
        Some(RepoStateBadge::Detached(sha)) => format!("[detached {}]", short_sha(sha)),
        Some(RepoStateBadge::Unborn(name)) => format!("[unborn {name}]"),
        None => "[state unknown]".into(),
    }
}

/// Render the `[<n> bytes]` badge for a buffer row. Falls back to
/// `[size unknown]` when the `buffer/byte-size` fact hasn't landed
/// yet (partial-bootstrap window — user visible only very briefly).
fn format_byte_size(size: Option<u64>) -> String {
    match size {
        Some(n) => format!("[{n} bytes]"),
        None => "[size unknown]".into(),
    }
}

/// Dirty indicator, or the observability-lost badge when the repo is
/// flagged unobservable. Per contract, dirty state is suppressed
/// while `repo/observable = false`.
fn format_dirty_or_obs_lost(observable: Option<bool>, dirty: Option<bool>) -> String {
    if observable == Some(false) {
        return "[observability lost]".into();
    }
    match dirty {
        Some(true) => "dirty".into(),
        Some(false) => "clean".into(),
        None => "".into(),
    }
}

/// Truncate a commit SHA for display. `git` uses 7 chars by
/// convention; the contract's sample rendering shows 8 + ellipsis.
///
/// F28 review fix: bus fact strings are not type-constrained to
/// ASCII, so byte-slicing `&sha[..DISPLAY_CHARS]` would panic on
/// a non-ASCII string whose 8th byte lands mid-codepoint. Use a
/// char iterator that peeks one past the truncation point to
/// decide whether to append the ellipsis — O(DISPLAY_CHARS) in
/// the common case, and safe on any UTF-8 input.
fn short_sha(sha: &str) -> String {
    const DISPLAY_CHARS: usize = 8;
    let mut iter = sha.chars();
    let prefix: String = iter.by_ref().take(DISPLAY_CHARS).collect();
    if iter.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn format_value(v: &FactValue) -> String {
    match v {
        FactValue::Bool(b) => b.to_string(),
        FactValue::String(s) => format!("{s:?}"),
        FactValue::Int(n) => n.to_string(),
        FactValue::U64(n) => n.to_string(),
        FactValue::Null => "null".into(),
    }
}

fn annotation(fact: &Fact, asserted_at_wall_ns: u64, stale: bool) -> String {
    let behavior = match &fact.provenance.source {
        ActorIdentity::Behavior { id } => id.as_str().to_string(),
        ActorIdentity::Core => "core".into(),
        ActorIdentity::Tui => "tui".into(),
        ActorIdentity::Service {
            service_id,
            instance_id,
        } => {
            // Show the service + a short instance suffix per
            // contracts/cli-surfaces.md TUI rendering rules.
            let inst = instance_id.as_hyphenated().to_string();
            let short = inst.get(..8).unwrap_or(inst.as_str());
            format!("service {service_id} (inst {short})")
        }
        ActorIdentity::User { id } => format!("user {id}"),
        ActorIdentity::Host { host_id, .. } => format!("host {host_id}"),
        ActorIdentity::Agent { agent_id, .. } => format!("agent {agent_id}"),
    };
    let event = match fact.provenance.causal_parent {
        Some(EventId { .. }) => format!("event {}", fact.provenance.causal_parent.unwrap()),
        None => "no causal parent".into(),
    };
    let age = age_label(asserted_at_wall_ns, stale);
    format!("by {behavior}, {event}, {age}")
}

fn age_label(asserted_at_wall_ns: u64, stale: bool) -> String {
    if stale {
        return "last seen before disconnect".into();
    }
    let now = wall_ns();
    let delta_ns = now.saturating_sub(asserted_at_wall_ns);
    let secs = (delta_ns as f64) / 1_000_000_000.0;
    if secs < 1.0 {
        format!("{:.3}s ago", secs)
    } else {
        format!("{:.1}s ago", secs)
    }
}

fn wall_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// RAII guard: disables raw mode and leaves the alternate screen on
/// drop so a panic doesn't leave the user's terminal in a broken state.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen,);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use weaver_core::provenance::Provenance;

    fn mk_fact(entity: EntityRef, attr: &str, value: FactValue) -> FactDisplay {
        mk_fact_at(entity, attr, value, 0)
    }

    fn mk_fact_at(entity: EntityRef, attr: &str, value: FactValue, ts: u64) -> FactDisplay {
        let identity = ActorIdentity::service("git-watcher", Uuid::new_v4()).unwrap();
        let provenance = Provenance::new(identity, 0, None).unwrap();
        FactDisplay {
            fact: Fact {
                key: FactKey::new(entity, attr),
                value,
                provenance,
            },
            asserted_at_wall_ns: ts,
        }
    }

    fn insert(facts: &mut HashMap<FactKey, FactDisplay>, fd: FactDisplay) {
        facts.insert(fd.fact.key.clone(), fd);
    }

    #[test]
    fn is_repo_fact_attribute_matches_repo_family_only() {
        assert!(is_repo_fact_attribute("repo/dirty"));
        assert!(is_repo_fact_attribute("repo/state/on-branch"));
        assert!(is_repo_fact_attribute("repo/head-commit"));
        assert!(!is_repo_fact_attribute("buffer/dirty"));
        assert!(!is_repo_fact_attribute("watcher/status"));
        assert!(!is_repo_fact_attribute("repository/dirty"));
    }

    #[test]
    fn state_badge_renders_each_variant() {
        assert_eq!(
            format_state_badge(Some(&RepoStateBadge::OnBranch("main"))),
            "[on main]"
        );
        assert_eq!(
            format_state_badge(Some(&RepoStateBadge::Detached(
                "abcdef0123456789abcdef0123456789abcdef01"
            ))),
            "[detached abcdef01...]"
        );
        assert_eq!(
            format_state_badge(Some(&RepoStateBadge::Unborn("main"))),
            "[unborn main]"
        );
        assert_eq!(format_state_badge(None), "[state unknown]");
    }

    #[test]
    fn is_buffer_fact_attribute_matches_family_prefix() {
        assert!(is_buffer_fact_attribute("buffer/path"));
        assert!(is_buffer_fact_attribute("buffer/byte-size"));
        assert!(is_buffer_fact_attribute("buffer/dirty"));
        assert!(is_buffer_fact_attribute("buffer/observable"));
        assert!(!is_buffer_fact_attribute("repo/dirty"));
        assert!(!is_buffer_fact_attribute("watcher/status"));
        assert!(!is_buffer_fact_attribute("buffers/dirty"));
    }

    #[test]
    fn format_byte_size_renders_with_fallback() {
        assert_eq!(format_byte_size(Some(0)), "[0 bytes]");
        assert_eq!(format_byte_size(Some(18342)), "[18342 bytes]");
        assert_eq!(format_byte_size(None), "[size unknown]");
    }

    #[test]
    fn collect_buffer_views_groups_by_entity_and_picks_freshest_representative() {
        let mut facts: HashMap<FactKey, FactDisplay> = HashMap::new();
        // Two buffers under distinct entities.
        insert(
            &mut facts,
            mk_fact_at(
                EntityRef::new(10),
                "buffer/path",
                FactValue::String("/tmp/a".into()),
                100,
            ),
        );
        insert(
            &mut facts,
            mk_fact_at(
                EntityRef::new(10),
                "buffer/byte-size",
                FactValue::U64(42),
                200,
            ),
        );
        insert(
            &mut facts,
            mk_fact_at(
                EntityRef::new(10),
                "buffer/dirty",
                FactValue::Bool(true),
                900,
            ),
        );
        insert(
            &mut facts,
            mk_fact_at(
                EntityRef::new(20),
                "buffer/path",
                FactValue::String("/tmp/b".into()),
                300,
            ),
        );
        insert(
            &mut facts,
            mk_fact_at(
                EntityRef::new(20),
                "buffer/observable",
                FactValue::Bool(false),
                400,
            ),
        );

        let views = collect_buffer_views(&facts);
        assert_eq!(views.len(), 2);
        // Deterministic entity ordering.
        assert_eq!(views[0].path, Some("/tmp/a"));
        assert_eq!(views[0].byte_size, Some(42));
        assert_eq!(views[0].dirty, Some(true));
        // Freshest fact (dirty @ t=900) is the representative.
        let (_, rep_ts) = views[0].representative.expect("representative");
        assert_eq!(rep_ts, 900);

        assert_eq!(views[1].path, Some("/tmp/b"));
        assert_eq!(views[1].observable, Some(false));
        assert_eq!(views[1].dirty, None);
    }

    #[test]
    fn dirty_or_obs_lost_prefers_observability_badge() {
        assert_eq!(
            format_dirty_or_obs_lost(Some(false), Some(true)),
            "[observability lost]",
            "observable=false must suppress dirty state per contract"
        );
        assert_eq!(format_dirty_or_obs_lost(Some(true), Some(true)), "dirty");
        assert_eq!(format_dirty_or_obs_lost(Some(true), Some(false)), "clean");
        assert_eq!(format_dirty_or_obs_lost(None, Some(true)), "dirty");
        assert_eq!(format_dirty_or_obs_lost(Some(true), None), "");
    }

    #[test]
    fn short_sha_truncates_at_eight_chars() {
        assert_eq!(short_sha("abcdef0123456789abcdef"), "abcdef01...");
        // Short strings pass through untouched (defensive — the
        // watcher always publishes full hex, but an operator running
        // `weaver status` could see shorter values in the future).
        assert_eq!(short_sha("abc"), "abc");
    }

    #[test]
    fn short_sha_handles_utf8_without_panic() {
        // F28 regression: bus fact strings are not type-constrained
        // to ASCII. A malformed or future-nonstandard publisher
        // could send a multi-byte codepoint whose byte position 8
        // lands mid-codepoint; byte-slicing would panic. Char-
        // iterator-based truncation must not panic and must
        // produce a valid UTF-8 string.
        let four_byte = "𝔸"; // 4-byte codepoint
        let input = four_byte.repeat(12); // 48 bytes, 12 chars
        let out = short_sha(&input);
        // 8 chars of 𝔸 + ellipsis.
        assert_eq!(out, format!("{}...", four_byte.repeat(8)));
        // Short string passes through.
        assert_eq!(short_sha("ß"), "ß");
    }

    #[test]
    fn collect_repo_views_picks_freshest_fact_as_representative() {
        // F27 regression: `representative` must be the fact with
        // the highest `asserted_at_wall_ns`, not whichever one
        // happened to iterate first. Prior logic could pick
        // `repo/path` (asserted once at startup) over
        // `repo/state/*` (refreshed every poll).
        let mut facts = HashMap::new();
        let e = EntityRef::new(1);
        // Old startup fact.
        insert(
            &mut facts,
            mk_fact_at(e, "repo/path", FactValue::String("/x".into()), 100),
        );
        // Fresh state fact — must win.
        insert(
            &mut facts,
            mk_fact_at(
                e,
                "repo/state/on-branch",
                FactValue::String("main".into()),
                900,
            ),
        );
        insert(
            &mut facts,
            mk_fact_at(e, "repo/dirty", FactValue::Bool(false), 500),
        );
        let views = collect_repo_views(&facts);
        assert_eq!(views.len(), 1);
        let (fact, ts) = views[0].representative.unwrap();
        assert_eq!(ts, 900);
        assert_eq!(fact.key.attribute, "repo/state/on-branch");
    }

    #[test]
    fn non_repo_keys_sort_by_entity_then_attribute() {
        // Catch-all Facts section: excludes both repo/* and buffer/*
        // — those have dedicated sections in the render output.
        let mut state = AppState::new();
        insert(
            &mut state.facts,
            mk_fact(EntityRef::new(2), "watcher/status", FactValue::Bool(true)),
        );
        insert(
            &mut state.facts,
            mk_fact(EntityRef::new(1), "watcher/status", FactValue::Bool(false)),
        );
        insert(
            &mut state.facts,
            mk_fact(EntityRef::new(1), "misc/probe", FactValue::Bool(true)),
        );
        // repo/* and buffer/* must be filtered out entirely — they
        // render in Repositories / Buffers, not in the catch-all Facts.
        insert(
            &mut state.facts,
            mk_fact(EntityRef::new(1), "repo/dirty", FactValue::Bool(false)),
        );
        insert(
            &mut state.facts,
            mk_fact(EntityRef::new(1), "buffer/dirty", FactValue::Bool(true)),
        );
        let keys = non_repo_fact_keys_sorted(&state);
        let attrs: Vec<&str> = keys.iter().map(|k| k.attribute.as_str()).collect();
        assert_eq!(
            attrs,
            vec!["misc/probe", "watcher/status", "watcher/status"]
        );
        assert_eq!(keys[0].entity.as_u64(), 1);
        assert_eq!(keys[1].entity.as_u64(), 1);
        assert_eq!(keys[2].entity.as_u64(), 2);
    }

    #[test]
    fn inspect_target_prefers_first_catch_all_fact() {
        // Catch-all Facts wins over Repositories and Buffers.
        let mut state = AppState::new();
        insert(
            &mut state.facts,
            mk_fact(EntityRef::new(5), "buffer/dirty", FactValue::Bool(true)),
        );
        insert(
            &mut state.facts,
            mk_fact(
                EntityRef::new(9),
                "repo/state/on-branch",
                FactValue::String("main".into()),
            ),
        );
        insert(
            &mut state.facts,
            mk_fact(EntityRef::new(3), "watcher/status", FactValue::Bool(true)),
        );
        let k = inspect_target(&state).expect("some target");
        assert_eq!(k.attribute, "watcher/status");
        assert_eq!(k.entity.as_u64(), 3);
    }

    #[test]
    fn inspect_target_falls_through_to_first_buffer_when_no_catch_all_or_repo() {
        // Only buffer/* facts — Repositories priority already exhausted
        // in the preceding catch-all-empty branch; buffer/* is the
        // last resort.
        let mut state = AppState::new();
        insert(
            &mut state.facts,
            mk_fact_at(
                EntityRef::new(42),
                "buffer/path",
                FactValue::String("/tmp/a".into()),
                100,
            ),
        );
        insert(
            &mut state.facts,
            mk_fact_at(
                EntityRef::new(42),
                "buffer/dirty",
                FactValue::Bool(false),
                900,
            ),
        );
        let k = inspect_target(&state).expect("some target");
        assert_eq!(k.attribute, "buffer/dirty");
        assert_eq!(k.entity.as_u64(), 42);
    }

    #[test]
    fn inspect_target_falls_through_to_first_repo_representative() {
        let mut state = AppState::new();
        // Only repo facts present.
        insert(
            &mut state.facts,
            mk_fact_at(
                EntityRef::new(42),
                "repo/state/on-branch",
                FactValue::String("main".into()),
                900,
            ),
        );
        insert(
            &mut state.facts,
            mk_fact_at(
                EntityRef::new(42),
                "repo/path",
                FactValue::String("/x".into()),
                100,
            ),
        );
        let k = inspect_target(&state).expect("some target");
        // The freshest (state fact at t=900) is the representative.
        assert_eq!(k.attribute, "repo/state/on-branch");
    }

    #[test]
    fn inspect_target_none_when_empty() {
        let state = AppState::new();
        assert!(inspect_target(&state).is_none());
    }

    #[test]
    fn collect_repo_views_groups_facts_by_entity() {
        let mut facts = HashMap::new();
        let e1 = EntityRef::new(10);
        let e2 = EntityRef::new(20);
        insert(
            &mut facts,
            mk_fact(e1, "repo/path", FactValue::String("/a".into())),
        );
        insert(
            &mut facts,
            mk_fact(e1, "repo/state/on-branch", FactValue::String("main".into())),
        );
        insert(&mut facts, mk_fact(e1, "repo/dirty", FactValue::Bool(true)));
        insert(
            &mut facts,
            mk_fact(e2, "repo/path", FactValue::String("/b".into())),
        );
        insert(
            &mut facts,
            mk_fact(e2, "repo/observable", FactValue::Bool(false)),
        );
        // A non-repo fact must be ignored.
        insert(
            &mut facts,
            mk_fact(EntityRef::new(99), "buffer/dirty", FactValue::Bool(true)),
        );

        let views = collect_repo_views(&facts);
        assert_eq!(views.len(), 2);
        let v0 = &views[0];
        assert_eq!(v0.path, Some("/a"));
        assert!(matches!(v0.state, Some(RepoStateBadge::OnBranch("main"))));
        assert_eq!(v0.dirty, Some(true));
        assert_eq!(v0.observable, None);
        assert!(v0.representative.is_some());
        let v1 = &views[1];
        assert_eq!(v1.path, Some("/b"));
        assert_eq!(v1.observable, Some(false));
    }
}
