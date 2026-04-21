# weaver-git-watcher

The first non-editor service actor on the Weaver bus. Observes one
local git repository and publishes authoritative `repo/*` facts
(dirty / head-commit / working-copy state) under a structured
`ActorIdentity::Service`.

## Role

`weaver-git-watcher` asserts the coordination-substrate pivot
(constitution v0.2 §17) at the code level: an actor Weaver has never
shipped before participates on the bus as a peer. See
`specs/002-git-watcher-actor/` for the full specification.

## Status

- Slice 002 Phase 1 (Setup): this crate compiles as a stub.
- Phase 3 (US1 MVP): CLI, observer, publisher, lifecycle — upcoming.

## Usage (once Phase 3 lands)

```bash
weaver-git-watcher /path/to/repository [--poll-interval=250ms] [--socket=/path/to/weaver.sock]
```

See `specs/002-git-watcher-actor/contracts/cli-surfaces.md` for the
full surface and `specs/002-git-watcher-actor/quickstart.md` for an
end-to-end three-process walkthrough.

## License

AGPL-3.0-or-later. See the top-level `LICENSE`.
