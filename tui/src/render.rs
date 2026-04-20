//! TUI render loop — crossterm raw-mode event loop that multiplexes
//! keystrokes, inbound bus messages, and disconnect detection.
//!
//! Implements tasks T047 (fact rendering), T072 (stale-on-disconnect),
//! and the display side of T046 (`e`/`c` keys wire through
//! [`crate::commands`]).
//!
//! Render surface matches `specs/001-hello-fact/contracts/cli-surfaces.md`:
//!
//! ```text
//! ┌─ Weaver TUI ────────────────────────────────────────────────────┐
//! │ Connection: ready (bus v0.1.0)                                  │
//! │                                                                 │
//! │ Facts:                                                          │
//! │   buffer/dirty(EntityRef(1)) = true                             │
//! │     by core/dirty-tracking, event N, 0.142s ago                 │
//! │                                                                 │
//! │ Commands: [e]dit  [c]lean  [i]nspect  [q]uit                    │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use std::collections::HashMap;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use crossterm::{cursor, execute, queue, style::Print, terminal};
use miette::{IntoDiagnostic, miette};
use tokio::sync::mpsc;

use weaver_core::provenance::SourceId;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;
use weaver_core::types::message::{
    BUS_PROTOCOL_VERSION_STR, BusMessage, InspectionDetail, InspectionError,
};

use crate::client::{BusStreamItem, connect};
use crate::commands::{self, SimulateKind};

/// The entity id used for every simulated event in slice 001. A
/// single-buffer fact space makes this constant safe.
const SYNTHETIC_BUFFER: EntityRef = EntityRef::new(1);

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
pub async fn run(socket: PathBuf) -> miette::Result<()> {
    let mut client = match connect(&socket).await {
        Ok(c) => c,
        Err(e) => {
            // Render-free short-circuit: we never entered raw mode, so
            // a plain-text error is appropriate.
            eprintln!("weaver-tui: {e}");
            eprintln!("[start `weaver run` in another terminal, then retry]");
            return Err(e);
        }
    };

    let mut state = AppState::new();

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
        tokio::select! {
            maybe_key = key_rx.recv() => {
                match maybe_key {
                    Some(key) => {
                        if should_quit(&key) {
                            break Ok(());
                        }
                        if state.is_available() {
                            if let Some(kind) = simulate_kind_for(&key) {
                                if let Err(e) =
                                    commands::publish(&mut client.writer, kind, SYNTHETIC_BUFFER)
                                        .await
                                {
                                    state.mark_unavailable(format!("write failed: {e}"));
                                }
                            } else if is_inspect_key(&key) {
                                // Inspect the first displayed fact, if any.
                                if let Some(target_key) = state.facts.keys().next().cloned() {
                                    match commands::inspect(
                                        &mut client.writer,
                                        target_key.clone(),
                                    )
                                    .await
                                    {
                                        Ok(request_id) => {
                                            state.pending_inspection = Some((request_id, target_key));
                                        }
                                        Err(e) => {
                                            state.mark_unavailable(format!("write failed: {e}"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    None => break Ok(()),
                }
            }
            maybe_msg = client.inbound.recv() => {
                match maybe_msg {
                    Some(item) => apply_inbound(&mut state, item),
                    None => state.mark_unavailable("bus reader ended".into()),
                }
            }
        }
        if let Err(e) = draw(&mut out, &state) {
            break Err(miette!("draw: {e}"));
        }
    };

    // Cleanup.
    key_task.abort();
    client.reader_task.abort();
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

fn simulate_kind_for(key: &KeyEvent) -> Option<SimulateKind> {
    // Ctrl-C is reserved for quit — handled in `should_quit`.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Char('e') | KeyCode::Char('E') => Some(SimulateKind::Edit),
        KeyCode::Char('c') | KeyCode::Char('C') => Some(SimulateKind::Clean),
        _ => None,
    }
}

fn is_inspect_key(key: &KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    matches!(key.code, KeyCode::Char('i') | KeyCode::Char('I'))
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
    if state.facts.is_empty() {
        emit(w, &mut row, "│   (none)".into())?;
    } else {
        for fd in state.facts.values() {
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
                emit(
                    w,
                    &mut row,
                    format!("│   asserting_behavior: {}", detail.asserting_behavior),
                )?;
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
        ConnStatus::Ready => emit(
            w,
            &mut row,
            "│ Commands: [e]dit  [c]lean  [i]nspect  [q]uit".into(),
        )?,
        ConnStatus::Unavailable { .. } => emit(w, &mut row, "│ Commands: [q]uit".into())?,
    }
    emit(
        w,
        &mut row,
        "└─────────────────────────────────────────────────────────────────┘".into(),
    )?;

    w.flush()?;
    Ok(())
}

fn format_value(v: &FactValue) -> String {
    match v {
        FactValue::Bool(b) => b.to_string(),
        FactValue::String(s) => format!("{s:?}"),
        FactValue::Int(n) => n.to_string(),
        FactValue::Null => "null".into(),
    }
}

fn annotation(fact: &Fact, asserted_at_wall_ns: u64, stale: bool) -> String {
    let behavior = match &fact.provenance.source {
        SourceId::Behavior(id) => id.as_str().to_string(),
        SourceId::Core => "core".into(),
        SourceId::Tui => "tui".into(),
        SourceId::External(s) => format!("external:{s}"),
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
