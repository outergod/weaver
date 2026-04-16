# Weaver Technology Considerations

## Core

Being the obvious choice for this kind of endeavor, Rust should be the main candidate for the Core because of the language and runtime traits:

- Cross-platform
- Highly performant
- Largely memory-safe

## UI

Web-based, but OS-native browser; probably Tauri to get the best of all worlds (full control without Electron bloat). CodeMirror, unless a better alternative exists.
