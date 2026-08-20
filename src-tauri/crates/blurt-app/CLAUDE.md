This crate is the orchestration layer — Tauri commands calling into the
domain crates. Keep it thin; business logic belongs in the domain crates,
not here. Read ../../../docs/MODULE_01_ARCHITECTURE.md §3 for the intended
wiring (async runtime, LLM lifecycle manager, IPC contract, event-driven
frontend updates) before adding a command.
