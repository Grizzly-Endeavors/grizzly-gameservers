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

pub const SANDBOX_ARG: &str = "__sandboxed-shell";

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

const OUTPUT_CAP_BYTES: usize = 64 * 1024;

const UNREADABLE_ROOT_DIRS: [&str; 2] = ["proc", "sys"];

const SCRATCH_DIR: &str = "/tmp";
const NULL_DEVICE: &str = "/dev/null";

const SANDBOX_ABI: ABI = ABI::V3;

pub async fn run_command(data_root: &std::path::Path, command: &str) -> Result<ShellResponse> {
    let exe = std::env::current_exe()
        .context("failed to locate the supervisor binary to sandbox the shell with")?;
    let mut child = Command::new(exe)
        .arg(SANDBOX_ARG)
        .arg(command)
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

pub fn exec_sandboxed(command: &str) -> Result<()> {
    enter_sandbox()?;
    Err(std::os::unix::process::CommandExt::exec(
        std::process::Command::new("/bin/sh").arg("-c").arg(command),
    ))
    .context("failed to exec /bin/sh for the sandboxed shell")
}

fn enter_sandbox() -> Result<()> {
    let abi = SANDBOX_ABI;
    let status = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .context("failed to declare the landlock access set")?
        .create()
        .context("failed to create the landlock ruleset")?
        .add_rule(PathBeneath::new(
            PathFd::new("/").context("failed to open / for the landlock ruleset")?,
            AccessFs::ReadDir,
        ))
        .context("failed to grant directory listing")?
        .add_rules(
            readable_roots(abi)?
                .into_iter()
                .map(Ok::<_, landlock::RulesetError>),
        )
        .context("failed to grant filesystem reads")?
        .add_rule(PathBeneath::new(
            PathFd::new(SCRATCH_DIR).with_context(|| {
                format!("failed to open {SCRATCH_DIR} for the landlock ruleset")
            })?,
            AccessFs::from_all(abi),
        ))
        .context("failed to grant scratch writes")?
        .add_rule(PathBeneath::new(
            PathFd::new(NULL_DEVICE).with_context(|| {
                format!("failed to open {NULL_DEVICE} for the landlock ruleset")
            })?,
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
        let access = match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => AccessFs::from_read(abi),
            Ok(_) => AccessFs::ReadFile | AccessFs::Execute,
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
