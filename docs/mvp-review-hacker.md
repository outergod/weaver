# MVP Review: Emacs Hacker Critique

## Persona Recap

Reviewer: Anders Vidvik, a 19-year GNU Emacs user and daily Elisp package author. He maintains MELPA packages, has contributed to Magit and Emacs core, edits Rust, Python, TypeScript, Go, and Common Lisp across many projects, and understands both why Emacs remains hard to replace and where it fails: blocking extension code, opaque hooks/advice, TRAMP uncertainty, Magit latency on large repositories, and poor introspection of runtime behavior.

This review treats Weaver as a possible first-customer system, not as an object of nostalgia. The question is not whether Weaver differs from Emacs. The question is whether its architectural bets can survive real editor workflows.

## 1. The jobs I do in Emacs daily that Weaver must handle

1. **Magit hunk work, not "git integration."** I run `magit-status`, stage individual hunks with `s`, split hunks when needed, commit with `c c`, amend/fixup, and rebase from the same interface. Weaver's docs mention a `diff hunk` as an entity candidate (`docs/01-system-model.md:20`) and "git-related actions" only as projected menu availability (`docs/06-workflows.md:45`). That is not yet a Magit replacement; it is a menu demo.

2. **Async compile/test loops with explainable failure state.** I run `compile`, `recompile`, project-specific test commands, then jump through errors in `compilation-mode`. Weaver needs task entities, streaming output, cancellation, stale state, and error navigation. The bus has streams and cancellation (`docs/02-architecture.md:66`), and task runs are entity candidates (`docs/01-system-model.md:21`), but the MVP explicitly excludes shell execution and task runners (`docs/mvp.md:163`).

3. **Org-like mixed semantic buffers.** I live in Org: links, code blocks, narrowing, agenda-ish derived views, export, capture. Weaver's substrate/projection model is promising because text projections can carry structured annotations as first-class content (`docs/01-system-model.md:283`), but adoption depends on whether those annotations can support real mixed-mode workflows rather than decorative highlighting.

4. **Live editor programming without restarting.** I edit Elisp, re-evaluate a form, inspect a variable, remove advice, and continue. Weaver correctly treats behavior reload as a first-class workflow (`docs/03-interaction-model.md:110`) and requires live behavior redefinition without losing authoritative state (`docs/mvp.md:179`). That must feel like `eval-defun`, not like redeploying a service.

5. **Remote and degraded work.** TRAMP hurts, but I use it. Weaver's distributed-service model acknowledges latency and failure explicitly (`docs/00-constitution.md:88`, `docs/02-architecture.md:161`). That is the right problem. It becomes useful only if reconnect, replay, and stale-state semantics are nailed down.

## 2. Three deal-breakers

1. **No committed undo model.** The docs say it plainly: "No undo model is committed" and "users lose work within the first minute" (`docs/07-open-questions.md:170`). The MVP then excludes undo/redo (`docs/mvp.md:164`). I would not try Weaver as an editor until undo is resolved as architecture, not postponed as polish.

2. **User code can still freeze the composition lane.** Weaver wants to escape Emacs's "one bad bit of Lisp blocks the world," but the architecture says long pure Steel computation blocks other behaviors and authors must yield manually (`docs/02-architecture.md:252`, `docs/02-architecture.md:256`). L2 simultaneously says the host must survive a Steel infinite loop (`.specify/memory/constitution.md:95`). That contradiction matters. If my hook-equivalent can hang applicability, I am out.

3. **The bus is lossy by default where correctness may depend on history.** Back-pressure defaults to `drop-oldest` (`docs/02-architecture.md:74`), and the open questions already admit missed critical transitions and retractions are unresolved (`docs/07-open-questions.md:258`). Losing progress events is fine. Losing state transitions, retractions, or causal history breaks the whole `why?` promise.

## 3. Three deal-makers

1. **Mandatory provenance and `why?`.** Emacs is extensible but often opaque. Weaver makes opacity a defect (`docs/00-constitution.md:19`) and requires facts, events, and actions to be attributable (`docs/00-constitution.md:220`). The MVP's `why?` channel is concrete enough to matter: source, authority, contributing behaviors, causal chain (`docs/mvp.md:65`). This is the strongest pitch.

2. **Live reload with contained failure.** The two-phase reload rule is exactly the right scar tissue: parse, validate, stage, swap; failed reload leaves the prior behavior live (`docs/04-composition-model.md:219`). That is better than my current pile of half-evaluated Elisp and stale closures.

3. **Action entities as command vocabulary.** Making commands queryable facts rather than UI menu entries is a serious idea. The docs define stable action entities (`docs/01-system-model.md:235`) and make `M-x`-style discovery a query over the same action space as contextual menus (`docs/03-interaction-model.md:100`). That could beat both Emacs command discovery and modal leader-key forests.

## 4. Three architectural concerns

1. **Bet: the core orchestrates every multi-authority action.** The docs say the core orchestrates even when it owns neither side of the affected facts (`docs/02-architecture.md:289`, `docs/02-architecture.md:301`). This may fail by turning the core into a semantic bottleneck: git rebase, test rerun, formatter save, and language-code-action workflows all want domain-specific sequencing. Evidence that would change my mind: a nontrivial git workflow where the git service owns real domain semantics, the core owns applicability/invariants, and the trace remains legible without duplicating git logic in the core.

2. **Bet: eventual UI views plus lossy queues remain trustworthy.** UIs are materialized views over system state (`docs/00-constitution.md:151`), while queues may drop old messages (`docs/02-architecture.md:76`). That combination is acceptable only with strong snapshot/replay semantics. Otherwise the UI can show stale applicability and still look authoritative. Evidence that would change my mind: protocol-level "must deliver" classes for fact assertions/retractions, sequence gaps, snapshot recovery, and reconnect replay.

3. **Bet: Steel is glue, services are capability.** The docs deliberately make Steel the only composition language (`docs/04-composition-model.md:104`) and forbid direct filesystem/network/process access from Steel (`docs/02-architecture.md:238`). That protects the system, but it risks making every useful customization cross the "write a service" threshold. Evidence that would change my mind: service scaffolding that is actually close to `defun` ceremony, because the docs correctly admit that without it promotion erodes extension velocity (`docs/04-composition-model.md:116`).

## 5. Three things Weaver clearly gets right

1. **Facts are not abused for buffer contents.** The component distinction is good engineering. Buffer text, ropes, ASTs, and token streams are not small provenance-heavy propositions (`docs/01-system-model.md:99`). Keeping facts small while allowing typed components avoids the obvious "everything is a triple store" trap.

2. **User scratch is first-class but non-authoritative.** The two-lane model is better than both Emacs free-for-all mutation and locked-down extension systems. Scratch facts are observable but cannot shadow governed authority (`docs/00-constitution.md:207`, `docs/04-composition-model.md:173`). That is the right governance line.

3. **Failed behavior matching is part of debugging.** The docs do not only ask "why did this fire?" They also require "why did it not fire?" (`docs/04-composition-model.md:89`). The MVP preserves near-match records for evaluated behaviors that did not fire (`docs/mvp.md:38`). That is the hook-debugging feature Emacs never quite gave me.

## 6. Three unanswered questions

1. **What exactly is undo?** Is it content-component versioning, a history service, command inversion, or something else? The docs lean toward history service plus versioned content (`docs/07-open-questions.md:195`) but do not commit. I would ask the authors to resolve this before any editor-shaped MVP.

2. **When does point become semantic?** Cursor and selection are local by default, but many commands depend on point: completion, symbol actions, rename, hunk splitting, region formatting. The docs call this open (`docs/07-open-questions.md:201`) and defer shared cursor facts in MVP (`docs/mvp.md:165`). What is the exact promotion API from client-local point to shared fact?

3. **What is the trace retention contract?** The trace is append-only (`docs/02-architecture.md:264`), but retention and compaction are unresolved (`docs/07-open-questions.md:147`). If `why?` has a horizon, say so in the protocol. If it does not, explain storage and migration. Historical explainability cannot be a vibe.

## 7. Suggested amendments

1. **Amend `docs/07-open-questions.md section 18` and `docs/mvp.md` to make undo a pre-editor gate.** Keep it out of the ontology prototype if necessary, but name the prototype honestly. Anything advertised as an editor MVP needs undo/redo architecture resolved before users type real text.

2. **Amend `docs/02-architecture.md section 3.1` and `docs/05-protocols.md` with delivery classes.** Separate lossy telemetry/progress from authoritative state transitions. Fact assertions, fact retractions, lifecycle changes, and causal trace links need sequence numbers, gap detection, and snapshot/replay recovery.

3. **Amend `docs/02-architecture.md section 9.4` to require Steel interruption/resource enforcement.** "Authors are responsible for yielding" (`docs/02-architecture.md:256`) is Emacs's old wound with better nouns. Make CPU budgets, cancellation, or VM interruption part of the composition contract.

4. **Add one serious workflow to `docs/06-workflows.md`: hunk-stage-to-commit.** The current git workflow only proves action projection (`docs/06-workflows.md:45`). Add diff hunk entities, staged/unstaged facts, split/stage/unstage/commit actions, and `why?` on a hunk action. If Weaver can survive that, I will pay attention.

## 8. Adoption verdict

I would try Weaver when the prototype proves live Steel reload, `why?`, action entities, and bus-only UI state in one running session, plus a resolved undo design on paper. I would switch only after it handles Magit-grade hunk staging, async compile/test navigation, reliable undo/redo, and hostile or broken user code without freezing the editor. Given the current documentation-only state, a serious evaluation is plausibly 6-12 months after the ontology MVP; primary editing is multiple years away unless the service-scaffolding and undo decisions land unusually well.
