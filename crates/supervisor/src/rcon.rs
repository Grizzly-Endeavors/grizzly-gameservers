//! RCON client for the in-pod `POST /command` route: connects to the game's
//! localhost RCON port and runs a single console command per call, in the
//! [`RconDialect`] the per-game config selects. Minecraft and Palworld return a
//! single response packet; a correct Source-engine server (Factorio) may fragment
//! a large reply across packets, terminated here with the Valve mirror-packet
//! sentinel. Palworld advertises Source but doesn't mirror the sentinel — so it
//! reads as single-packet, not fragmented (see [`RconDialect::Palworld`]).
//!
//! The password is minted once at pod startup ([`RconRuntime::new`]) from the
//! system CSPRNG and injected into the game child's environment by the runner, so
//! it never touches git or a Kubernetes object and rotates every pod start.
//!
//! Source RCON protocol reference: <https://developer.valvesoftware.com/wiki/Source_RCON_Protocol>.

use std::fmt;
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tracing::debug;

const RCON_TIMEOUT: Duration = Duration::from_secs(15);

const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// Cap on the reply text returned to the bot. Console replies are normally short;
/// this guards the Discord relay path against a pathological dump (e.g. `help`).
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

const PASSWORD_BYTES: usize = 24;

/// Packet type ids from the Source RCON protocol. `EXECCOMMAND` and
/// `AUTH_RESPONSE` share the value 2 — they're disambiguated by the phase, not
/// the wire, so both constants exist for readability at their call sites.
const TYPE_RESPONSE_VALUE: i32 = 0;
const TYPE_EXECCOMMAND: i32 = 2;
const TYPE_AUTH_RESPONSE: i32 = 2;
const TYPE_AUTH: i32 = 3;

const ID_AUTH: i32 = 1;
const ID_EXEC: i32 = 2;
const ID_SENTINEL: i32 = 3;
const ID_AUTH_FAILED: i32 = -1;

const MIN_PACKET_LEN: usize = 10;
const MAX_PACKET_LEN: usize = 64 * 1024;
/// Source caps a request packet near 4 KiB; keep command bodies well under it.
const MAX_COMMAND_LEN: usize = 4000;

/// Which console dialect a game speaks over RCON. Bundles two traits that a game
/// fixes together: how a command reply is framed on the wire, and which command
/// vocabulary the game understands. Selected per game via `SUPERVISOR_RCON_DIALECT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RconDialect {
    Minecraft,
    /// A correct Source RCON implementation (Factorio): a large reply may fragment
    /// across packets, ended by mirroring an empty sentinel; `say` broadcasts; no
    /// snapshot-quiesce verbs.
    Source,
    /// Palworld's partial Source implementation: it replies in a single packet and
    /// does NOT mirror the sentinel (and drops the socket after replying), so the
    /// Source fragmented read would hang until timeout/EOF and surface a false
    /// failure even though the command ran — read one packet like Minecraft.
    /// Broadcasts use Palworld's own `Broadcast` verb; no quiesce verbs.
    Palworld,
    /// Valheim via the `ValheimRcon` `BepInEx` mod: standard Source packet framing but
    /// a single response packet with no sentinel mirror (like Minecraft/Palworld,
    /// so the fragmented read would hang). Broadcasts with `say`; the `players`
    /// command opens with an "Online N" line; no quiesce verbs.
    Valheim,
}

impl RconDialect {
    fn single_packet_reply(self) -> bool {
        matches!(self, Self::Minecraft | Self::Palworld | Self::Valheim)
    }

    fn supports_save_quiesce(self) -> bool {
        matches!(self, Self::Minecraft)
    }

    fn player_count_command(self) -> &'static str {
        match self {
            Self::Minecraft => "list",
            // Factorio: `/players` lists everyone; the online ones carry an
            // " (online)" suffix (mirrors upstream's players-online.sh).
            Self::Source => "/players",
            Self::Palworld => "ShowPlayers",
            Self::Valheim => "players",
        }
    }

    /// Parse the online player count out of a [`Self::player_count_command`]
    /// reply. `None` means the reply couldn't be understood — the caller treats
    /// that as "occupancy unknown", never as "empty", so a garbled reply can't
    /// trigger a disruptive action.
    fn parse_player_count(self, reply: &str) -> Option<u32> {
        match self {
            // "There are N of a max of M players online: ..." — N is the count.
            // Valheim's ValheimRcon opens its reply with "Online N" — same shape.
            Self::Minecraft | Self::Valheim => first_integer(reply),
            Self::Source => Some(count_online_suffixed(reply)),
            Self::Palworld => Some(parse_palworld_player_count(reply)),
        }
    }
}

fn first_integer(text: &str) -> Option<u32> {
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn count_online_suffixed(reply: &str) -> u32 {
    let count = reply
        .lines()
        .filter(|line| line.trim_end().ends_with("(online)"))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn parse_palworld_player_count(reply: &str) -> u32 {
    let rows = reply
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .count();
    u32::try_from(rows).unwrap_or(u32::MAX)
}

impl std::str::FromStr for RconDialect {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "minecraft" => Ok(Self::Minecraft),
            "source" => Ok(Self::Source),
            "palworld" => Ok(Self::Palworld),
            "valheim" => Ok(Self::Valheim),
            other => {
                bail!(
                    "unknown rcon dialect {other:?}; expected minecraft, source, palworld, or valheim"
                )
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ConnectMode {
    Retry,
    Once,
}

pub struct RconRuntime {
    port: u16,
    password: String,
    dialect: RconDialect,
}

impl RconRuntime {
    /// Build a runtime for `port` speaking `dialect`, minting a fresh random
    /// password. When `max_password_len` is `Some(n)`, the minted password is
    /// truncated to `n` characters — for games that cap their RCON/admin password
    /// (Palworld's 30-char `ADMIN_PASSWORD`). The password is lowercase hex
    /// (ASCII), so truncating at any character index stays on a valid boundary, and
    /// the same truncated value is both injected into the game and used to
    /// authenticate.
    ///
    /// # Errors
    ///
    /// Returns an error if the system random source can't be read.
    pub fn new(port: u16, dialect: RconDialect, max_password_len: Option<usize>) -> Result<Self> {
        let mut password = generate_password()?;
        if let Some(max) = max_password_len
            && password.len() > max
        {
            password.truncate(max);
        }
        Ok(Self {
            port,
            password,
            dialect,
        })
    }

    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }

    pub async fn run_command(&self, command: &str) -> Result<String> {
        self.run(command, ConnectMode::Retry).await
    }

    async fn run(&self, command: &str, connect: ConnectMode) -> Result<String> {
        self.run_within(command, connect, RCON_TIMEOUT).await
    }

    async fn run_within(
        &self,
        command: &str,
        connect: ConnectMode,
        budget: Duration,
    ) -> Result<String> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let connected = AtomicBool::new(false);
        match timeout(budget, self.exec(address, command, connect, &connected)).await {
            Ok(output) => Ok(truncate_output(output?)),
            Err(_elapsed) => Err(deadline_error(
                address,
                connected.load(Ordering::Relaxed),
                budget,
            )),
        }
    }

    pub async fn broadcast(&self, message: &str) -> Result<String> {
        self.run_command(&broadcast_command(message, self.dialect)?)
            .await
    }

    pub async fn player_count(&self) -> Option<u32> {
        let command = self.dialect.player_count_command();
        match self.run(command, ConnectMode::Once).await {
            Ok(reply) => {
                let count = self.dialect.parse_player_count(&reply);
                if count.is_none() {
                    debug!(
                        dialect = ?self.dialect,
                        reply = %reply,
                        "could not parse a player count from the rcon reply"
                    );
                }
                count
            }
            Err(err) => {
                debug!(error = ?err, "rcon player-count query did not complete");
                None
            }
        }
    }

    pub async fn quiesce_for_snapshot(&self) -> Result<()> {
        if !self.dialect.supports_save_quiesce() {
            return Ok(());
        }
        self.run_command("save-off").await?;
        self.run_command("save-all flush").await?;
        Ok(())
    }

    pub async fn resume_saves(&self) -> Result<()> {
        if !self.dialect.supports_save_quiesce() {
            return Ok(());
        }
        self.run_command("save-on").await?;
        Ok(())
    }

    async fn exec(
        &self,
        address: SocketAddr,
        command: &str,
        connect: ConnectMode,
        connected: &AtomicBool,
    ) -> Result<String> {
        if command.len() > MAX_COMMAND_LEN {
            bail!("command is too long for rcon ({} bytes)", command.len());
        }
        let mut stream = match connect {
            ConnectMode::Retry => connect_with_retry(address).await?,
            ConnectMode::Once => TcpStream::connect(address)
                .await
                .with_context(|| format!("nothing is listening on the game rcon port {address}"))?,
        };
        connected.store(true, Ordering::Relaxed);
        authenticate(&mut stream, &self.password).await?;
        write_packet(&mut stream, ID_EXEC, TYPE_EXECCOMMAND, command).await?;
        if self.dialect.single_packet_reply() {
            // Minecraft, Palworld, and Valheim reply with a single response packet;
            // read just that. Palworld and Valheim additionally never mirror the
            // sentinel, so attempting the fragmented read would hang until
            // timeout/EOF on a command that already ran — the false "communication
            // hiccup" this avoids.
            let packet = read_packet(&mut stream)
                .await
                .context("failed to read rcon command response")?;
            Ok(packet.body)
        } else {
            read_fragmented_response(&mut stream).await
        }
    }
}

impl fmt::Debug for RconRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RconRuntime")
            .field("port", &self.port)
            .field("password", &"<redacted>")
            .field("dialect", &self.dialect)
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Packet {
    id: i32,
    kind: i32,
    body: String,
}

/// Build the console command that broadcasts `message` to all players, in the
/// game's dialect. Minecraft uses `tellraw @a` with a JSON text component (built
/// with `serde_json` so the message is escaped, never hand-quoted); Palworld uses
/// its own `Broadcast` verb; a plain Source game falls back to `say`.
///
/// # Errors
///
/// Returns an error only if the Minecraft message can't be encoded as JSON.
fn broadcast_command(message: &str, dialect: RconDialect) -> Result<String> {
    match dialect {
        RconDialect::Minecraft => {
            let component = serde_json::to_string(&serde_json::json!({
                "text": message,
                "color": "yellow",
            }))
            .context("failed to encode the tellraw message")?;
            Ok(format!("tellraw @a {component}"))
        }
        // Palworld's Broadcast is known to mangle spaces in some client builds; the
        // verb itself is still correct and the bot's messages are short.
        RconDialect::Palworld => Ok(format!("Broadcast {message}")),
        RconDialect::Source | RconDialect::Valheim => Ok(format!("say {message}")),
    }
}

fn deadline_error(address: SocketAddr, connected: bool, budget: Duration) -> anyhow::Error {
    let secs = budget.as_secs();
    if connected {
        anyhow!(
            "rcon at {address} accepted the connection but sent no reply within {secs}s — \
             the game is running but its console is not responding"
        )
    } else {
        anyhow!(
            "nothing is listening on the game rcon port {address} — every connection was \
             refused for {secs}s; the game may still be starting, or its rcon plugin failed \
             to load (check the game logs)"
        )
    }
}

async fn connect_with_retry(address: SocketAddr) -> Result<TcpStream> {
    loop {
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(err) if err.kind() == ErrorKind::ConnectionRefused => {
                sleep(CONNECT_RETRY_INTERVAL).await;
            }
            Err(err) => {
                return Err(err).context("failed to connect to the game rcon port");
            }
        }
    }
}

/// Send the auth packet and read until the server's auth response, failing if it
/// rejects the password. Source precedes the auth response with an empty
/// `RESPONSE_VALUE`; Minecraft sends the auth response directly — reading until an
/// `AUTH_RESPONSE`-typed packet handles both.
async fn authenticate<S>(stream: &mut S, password: &str) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_packet(stream, ID_AUTH, TYPE_AUTH, password).await?;
    loop {
        let packet = read_packet(stream)
            .await
            .context("failed to read rcon auth response")?;
        if packet.kind == TYPE_AUTH_RESPONSE {
            if packet.id == ID_AUTH_FAILED {
                bail!("rcon authentication failed (wrong password?)");
            }
            return Ok(());
        }
    }
}

async fn read_fragmented_response<S>(stream: &mut S) -> Result<String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_packet(stream, ID_SENTINEL, TYPE_RESPONSE_VALUE, "").await?;
    let mut output = String::new();
    loop {
        let packet = read_packet(stream)
            .await
            .context("failed to read rcon command response")?;
        if packet.id == ID_SENTINEL {
            return Ok(output);
        }
        if packet.id == ID_EXEC {
            output.push_str(&packet.body);
        }
    }
}

fn encode_packet(id: i32, kind: i32, body: &str) -> Result<Vec<u8>> {
    let body = body.as_bytes();
    let length = body
        .len()
        .checked_add(MIN_PACKET_LEN)
        .context("rcon packet body too large")?;
    let length_field = i32::try_from(length).context("rcon packet length exceeds i32")?;
    let mut buf = Vec::with_capacity(length + 4);
    buf.extend_from_slice(&length_field.to_le_bytes());
    buf.extend_from_slice(&id.to_le_bytes());
    buf.extend_from_slice(&kind.to_le_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(&[0, 0]);
    Ok(buf)
}

async fn write_packet<W>(writer: &mut W, id: i32, kind: i32, body: &str) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let packet = encode_packet(id, kind, body)?;
    writer
        .write_all(&packet)
        .await
        .context("failed to write rcon packet")?;
    writer.flush().await.context("failed to flush rcon packet")
}

async fn read_packet<R>(reader: &mut R) -> Result<Packet>
where
    R: AsyncRead + Unpin,
{
    let length =
        usize::try_from(read_le_i32(reader).await?).context("rcon packet length is negative")?;
    if !(MIN_PACKET_LEN..=MAX_PACKET_LEN).contains(&length) {
        bail!("rcon packet length {length} out of range");
    }
    let id = read_le_i32(reader).await?;
    let kind = read_le_i32(reader).await?;
    let remaining = length - 8;
    let mut rest = vec![0_u8; remaining];
    reader
        .read_exact(&mut rest)
        .await
        .context("failed to read rcon packet body")?;
    let body_len = remaining.saturating_sub(2);
    let body = String::from_utf8_lossy(rest.get(..body_len).unwrap_or(&[])).into_owned();
    Ok(Packet { id, kind, body })
}

async fn read_le_i32<R>(reader: &mut R) -> Result<i32>
where
    R: AsyncRead + Unpin,
{
    let mut buf = [0_u8; 4];
    reader
        .read_exact(&mut buf)
        .await
        .context("failed to read rcon field")?;
    Ok(i32::from_le_bytes(buf))
}

pub fn generate_password() -> Result<String> {
    let mut bytes = [0_u8; PASSWORD_BYTES];
    getrandom::fill(&mut bytes)
        .context("failed to read system random source for the rcon password")?;
    let mut password = String::with_capacity(PASSWORD_BYTES * 2);
    for byte in bytes {
        write!(password, "{byte:02x}").context("failed to format the rcon password")?;
    }
    Ok(password)
}

fn truncate_output(mut output: String) -> String {
    if output.len() <= MAX_OUTPUT_BYTES {
        return output;
    }
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    output.truncate(end);
    output.push_str("… (truncated)");
    output
}

#[cfg(test)]
#[path = "tests/rcon.rs"]
mod tests;
