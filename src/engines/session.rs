//! Engine sessions: one worker process per bridged engine, its state machine, the handshake,
//! the re-entrant request loop and shutdown (INTERNALS ch. 8). Arrives in roadmap 1.3; the
//! child-process rules and the watchdog it builds on are in `engines/process`.
