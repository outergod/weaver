# AGENTS.md

Start here before making changes.

## Project orientation
- Read `README.md` first.
- The repository structure is defined in `docs/09-repository-layout.md`.
- The conceptual project model lives in `docs/00-constitution.md` through `docs/08-technologies.md`.

## Placement rules
- Prefer extending an existing project/workspace before creating a new one.
- Keep UI concerns out of core unless explicitly part of a cross-layer protocol or model.

## Architecture discipline
- Treat docs in `docs/` as the architectural reference.
- When implementation conflicts with docs, flag the mismatch explicitly.

## Engineering discipline
- Engineering practices are codified in `.specify/memory/constitution.md` (L2).
- L1 (`docs/00-constitution.md`) binds *what Weaver is*; L2 binds *how it is built*. L1 supersedes L2 on conflict.
- AI agents bind to L2 — see Principle 21 (regression tests before fix, changelog on public-surface changes, attributable commits).

## Change workflow
- Non-trivial changes go through Spec Kit:
  - `/speckit.specify` — write a feature spec under `specs/[###-feature-name]/spec.md`.
  - `/speckit.plan` — derive an implementation plan; the Constitution Check gates it against L2.
  - `/speckit.tasks` — generate dependency-ordered tasks.
  - `/speckit.implement` — execute the task list.
- For trivial fixes (typos, single-line edits), edit directly but still follow Principle 10 (regression test before fix when applicable).

## Commit conventions
- Follow [Conventional Commits 1.0](https://www.conventionalcommits.org/): `<type>(<scope>): <description>`.
- **Type vocabulary**: `feat`, `fix`, `refactor`, `docs`, `chore`, `test`, `build`, `ci`, `perf`, `style`.
- **Scope vocabulary** (hybrid):
  - Public surfaces (Principle 7): `bus`, `steel`, `fact`, `action`, `cli`, `config`.
  - Workspace/area: `core`, `ui`, `tui`, `docs`, `specify`.
- Breaking public-surface changes MUST include a `BREAKING CHANGE:` footer (drives MAJOR bumps under Principle 8).
- **Examples**:
  - `feat(bus): add streaming response message type`
  - `fix(steel): prevent panic on malformed fact assertion`
  - `refactor(core): extract trace-store migration helpers`
  - `docs(specify): clarify retraction-task expectations`
  - `chore(ci): pin Steel toolchain version`

