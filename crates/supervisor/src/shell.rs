//! A read-only shell for the ops agent.
//!
//! Gary's typed file tools ([`crate::fs`]) are confined to the data root and can
//! only read a whole file from its front, which leaves him unable to answer
//! "what killed this launch" when the evidence is a stack trace two thousand
//! lines into a boot, or a loader log the game keeps outside the data root. A
//! shell is the answer because every model already knows how to drive one — no
//! bespoke search route comes close to `grep`.
//!
//! What makes that safe to hand over is the sandbox rather than the vocabulary.
//! Parsing a command line for "mutating" verbs is unwinnable (`sed -i`, a bare
//! `>`, a script invoked by name), so the restriction is enforced by the kernel:
//! the command runs under a Landlock ruleset that grants read and execute and
//! grants no write anywhere the game's state lives. That keeps two invariants
//! the platform already depends on:
//!
//! - **Every config change still takes a snapshot.** The bot arms its
//!   auto-rollback only for writes that reported one ([`crate::fs::write_file`]),
//!   so a config edited around that path would silently lose its rollback. Under
//!   the sandbox that edit is refused by the kernel, and mutation keeps going
//!   through the typed tools that snapshot.
//! - **Credentials stay out of the channel.** The supervisor's environment holds
//!   the control token and the game's RCON password, and `/proc/1/environ` would
//!   hand both to anyone who can ask Gary to read a file. Procfs and sysfs are
//!   therefore the one part of the filesystem the sandbox does not grant, so
//!   `ps` and `<(process substitution)` do not work here — the trade is
//!   deliberate.
//!
//! Landlock needs no capability (ADR-023 drops all but a bootstrap set, and
//! `SYS_ADMIN` — which a mount-namespace approach would have required — is not
//! among them). It needs `no_new_privs`, which the pods already set via
//! `allowPrivilegeEscalation: false`.
//!
//! The ruleset is applied by re-executing this binary with [`SANDBOX_ARG`] and
//! restricting *that* process before it becomes `sh`. Applying it in a
//! `pre_exec` closure would mean allocating between `fork` and `exec` in a
//! threaded runtime, which can deadlock on the allocator lock; a fresh process
//! has no such constraint.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use grizzly_control_api::ShellResponse;
use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus,
};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, warn};

/// Marker argument that puts a re-executed supervisor into sandbox-then-exec
/// mode. Double-underscored because it is an implementation detail of this
/// module, not a supported entrypoint: it grants nothing, it only ever narrows
/// what the resulting process may do.
pub const SANDBOX_ARG: &str = "__sandboxed-shell";

/// Wall-clock ceiling on one command. Long enough for a `grep` over a boot log,
/// short enough that a wedged command can't hold an agent turn open.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-stream cap on retained output. Past this the bytes are read and dropped
/// rather than kept, so `cat` on a world file can't grow the pod's memory.
const OUTPUT_CAP_BYTES: usize = 64 * 1024;

/// Top-level directories the sandbox grants no file reads under. See the module
/// docs: procfs carries the supervisor's own environment, and with it the
/// control token and the game's RCON password.
const UNREADABLE_ROOT_DIRS: [&str; 2] = ["proc", "sys"];

/// Where a sandboxed command may write. Neither can hold game state, so a
/// command that needs scratch space (`sort` spilling, say) still works without
/// the sandbox ceasing to mean anything.
const SCRATCH_DIR: &str = "/tmp";
const NULL_DEVICE: &str = "/dev/null";

/// The access vocabulary the ruleset is built from. v3 is the floor rather than
/// a preference: before it the kernel cannot restrict `truncate`, so a
/// "read-only" sandbox would still let a command empty a config file — worse
/// than no sandbox, because the guarantee would be stated and false. A kernel
/// that can't handle the whole set reports less than fully enforced, and
/// [`enter_sandbox`] refuses to continue.
const SANDBOX_ABI: ABI = ABI::V3;

/// Run one command under the sandbox and collect its capped output.
///
/// Never returns a transport error for a command that merely failed: a non-zero
/// exit is an ordinary result the agent reads. `Err` is reserved for the shell
/// being unable to run at all.
///
/// # Errors
///
/// Returns an error if the supervisor binary can't be located or re-executed, or
/// if the child's pipes can't be taken.
pub async fn run_command(data_root: &std::path::Path, command: &str) -> Result<ShellResponse> {
    // The rule this suppresses guards against trusting current_exe for a
    // privilege decision — a setuid-style path where the re-executed image gains
    // something. This one only ever sheds: same uid, same container, same mount
    // namespace, and the process it starts exists to drop privilege before
    // becoming `sh`. Substituting the binary requires write access inside the
    // container, which is root there (ADR-023 leaves that gap open knowingly),
    // and root in this container can already do everything this path could be
    // twisted into doing. Nothing is gained, so nothing is trusted.
    // nosemgrep: rust.lang.security.current-exe
    let exe = std::env::current_exe()
        .context("failed to locate the supervisor binary to sandbox the shell with")?;
    let mut child = Command::new(exe)
        .arg(SANDBOX_ARG)
        .arg(command)
        // The data root is where an agent's paths are relative to, so a bare
        // `grep -r foo .` means the thing he'd expect it to mean.
        .current_dir(data_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn the sandboxed shell")?;
    let stdout = child
        .stdout
        .take()
        .context("sandboxed shell was spawned without a stdout pipe")?;
    let stderr = child
        .stderr
        .take()
        .context("sandboxed shell was spawned without a stderr pipe")?;
    // The readers own the pipes on their own task so the timeout below can kill
    // the child without fighting a borrow: the kill closes the pipes, the
    // readers see EOF, and the join yields whatever had been captured.
    let streams =
        tokio::spawn(async move { tokio::join!(read_capped(stdout), read_capped(stderr)) });
    let waited = tokio::time::timeout(COMMAND_TIMEOUT, child.wait()).await;
    let (status, timed_out) = if let Ok(status) = waited {
        (
            status.context("failed waiting on the sandboxed shell")?,
            false,
        )
    } else {
        warn!(
            timeout_secs = COMMAND_TIMEOUT.as_secs(),
            "sandboxed shell command timed out; killing"
        );
        child
            .kill()
            .await
            .context("failed to kill a timed-out sandboxed shell")?;
        let status = child
            .wait()
            .await
            .context("failed reaping a timed-out sandboxed shell")?;
        (status, true)
    };
    let ((out_bytes, out_cut), (err_bytes, err_cut)) = streams
        .await
        .context("sandboxed shell output readers failed")?;
    Ok(ShellResponse {
        stdout: String::from_utf8_lossy(&out_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&err_bytes).into_owned(),
        exit_code: status.code(),
        truncated: out_cut || err_cut,
        timed_out,
    })
}

/// Read a stream, retaining at most [`OUTPUT_CAP_BYTES`] and discarding the
/// rest. Returns the retained bytes and whether anything was dropped.
///
/// Draining past the cap rather than stopping is deliberate: a reader that
/// simply stopped would fill the pipe and block the child until the timeout
/// killed it, turning a chatty command into a 30-second one.
async fn read_capped<R>(mut reader: R) -> (Vec<u8>, bool)
where
    R: AsyncReadExt + Unpin,
{
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut buf = [0_u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(read) => {
                let chunk = buf.get(..read).unwrap_or(&buf);
                let room = OUTPUT_CAP_BYTES.saturating_sub(kept.len());
                match chunk.get(..room) {
                    // More arrived than there was room for: keep the prefix that
                    // fits and drop the rest. `room >= chunk.len()` falls through
                    // to the arm below, so a chunk that exactly fills the cap is
                    // not reported as truncated when nothing was actually lost.
                    Some(fitting) if room < chunk.len() => {
                        kept.extend_from_slice(fitting);
                        truncated = true;
                    }
                    _ => kept.extend_from_slice(chunk),
                }
            }
            Err(err) => {
                debug!(error = ?err, "sandboxed shell stream ended on a read error");
                break;
            }
        }
    }
    (kept, truncated)
}

/// Apply the sandbox to the current process and become `sh -c command`.
///
/// Only returns on failure — a success replaces this process image.
///
/// # Errors
///
/// Returns an error if the kernel's Landlock support is too old or the ruleset
/// can't be enforced (both fail closed, because a shell that silently wasn't
/// sandboxed would be a write path with no snapshot behind it), or if `sh`
/// can't be executed.
pub fn exec_sandboxed(command: &str) -> Result<()> {
    enter_sandbox()?;
    // Only returns on failure; on success this process becomes the shell.
    Err(std::os::unix::process::CommandExt::exec(
        std::process::Command::new("/bin/sh").arg("-c").arg(command),
    ))
    .context("failed to exec /bin/sh for the sandboxed shell")
}

/// Restrict the current process to reading the filesystem, writing only the two
/// scratch paths, and reading no files under procfs or sysfs.
fn enter_sandbox() -> Result<()> {
    let abi = SANDBOX_ABI;
    let status = Ruleset::default()
        // Best effort here means the kernel may decline part of the set on an
        // old build; the enforcement check at the bottom is what turns that into
        // a refusal rather than a quietly weaker sandbox.
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .context("failed to declare the landlock access set")?
        .create()
        .context("failed to create the landlock ruleset")?
        // Listing `/` and `/proc` stays possible; reading files under the denied
        // roots does not, because ReadFile is granted per-directory below.
        .add_rule(
            PathBeneath::new(
                PathFd::new("/").context("failed to open / for the landlock ruleset")?,
                AccessFs::ReadDir,
            ),
        )
        .context("failed to grant directory listing")?
        .add_rules(readable_roots(abi)?.into_iter().map(Ok::<_, landlock::RulesetError>))
        .context("failed to grant filesystem reads")?
        .add_rule(PathBeneath::new(
            PathFd::new(SCRATCH_DIR)
                .with_context(|| format!("failed to open {SCRATCH_DIR} for the landlock ruleset"))?,
            AccessFs::from_all(abi),
        ))
        .context("failed to grant scratch writes")?
        .add_rule(PathBeneath::new(
            PathFd::new(NULL_DEVICE)
                .with_context(|| format!("failed to open {NULL_DEVICE} for the landlock ruleset"))?,
            AccessFs::WriteFile,
        ))
        .context("failed to grant the null device")?
        .restrict_self()
        .context("failed to apply the landlock sandbox")?;
    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(()),
        RulesetStatus::PartiallyEnforced | RulesetStatus::NotEnforced => bail!(
            "landlock could not fully enforce the shell sandbox on this kernel ({:?})",
            status.ruleset
        ),
    }
}

/// Read-and-execute rules for every top-level entry except the ones in
/// [`UNREADABLE_ROOT_DIRS`]. Built by walking `/` rather than from a fixed list
/// because the six game bases lay their filesystems out differently, and a
/// missing entry would show up as a mysteriously unreadable path.
///
/// Directories and files get different rights on purpose: Landlock treats
/// `ReadDir` on a non-directory as an unsupportable request and downgrades the
/// whole ruleset to partially-enforced, which [`enter_sandbox`] then refuses. A
/// root with a plain file in it (`/vmlinuz`, an image's stray marker file) would
/// otherwise take the shell down with it.
fn readable_root(name: &std::ffi::OsStr) -> bool {
    !UNREADABLE_ROOT_DIRS
        .iter()
        .any(|denied| name == std::ffi::OsStr::new(denied))
}

fn readable_roots(abi: ABI) -> Result<Vec<PathBeneath<PathFd>>> {
    let mut rules = Vec::new();
    for entry in std::fs::read_dir("/").context("failed to list / for the landlock ruleset")? {
        let entry = entry.context("failed to read an entry of / for the landlock ruleset")?;
        if !readable_root(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        // Follows symlinks, because /bin and friends are commonly links to
        // directories and the rights depend on what the path resolves to.
        let access = match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => AccessFs::from_read(abi),
            Ok(_) => AccessFs::ReadFile | AccessFs::Execute,
            // A dangling or unreadable entry is not a reason to refuse the whole
            // sandbox; it is simply a path the shell won't be able to read.
            Err(err) => {
                debug!(error = ?err, path = %path.display(), "skipping unstattable root entry");
                continue;
            }
        };
        match PathFd::new(&path) {
            Ok(fd) => rules.push(PathBeneath::new(fd, access)),
            Err(err) => {
                debug!(error = ?err, path = %path.display(), "skipping unopenable root entry");
            }
        }
    }
    Ok(rules)
}

#[cfg(test)]
#[path = "tests/shell.rs"]
mod tests;
