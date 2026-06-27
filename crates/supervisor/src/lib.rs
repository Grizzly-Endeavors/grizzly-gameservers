//! In-pod process supervisor for game-server containers.
//!
//! Runs as the container entrypoint: launches the game server as a child
//! process, owns its lifecycle (graceful stop, in-place restart, crash
//! relaunch), keeps the Agones SDK health heartbeat alive even while the game is
//! intentionally paused, and exposes an HTTP control API the Discord bot drives.
//!
//! The pure decision logic ([`config`], [`state`]) is separated from the IO
//! shell ([`process`], [`readiness`], [`sdk`], [`control`], [`runner`]) so the
//! state machine is unit-testable without spawning processes or opening sockets.

pub mod config;
pub mod control;
pub mod process;
pub mod readiness;
pub mod runner;
pub mod sdk;
pub mod state;
