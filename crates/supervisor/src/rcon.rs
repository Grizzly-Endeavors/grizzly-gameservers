//! RCON client for the in-pod `POST /command` route: connects to the game's
//! localhost RCON port and runs a single console command per call, in the dialect
//! the per-game config selects. Minecraft's RCON returns a single response packet
//! (`minecraft_quirks`); Source-engine servers may fragment a large reply across
//! packets, terminated here with the Valve mirror-packet sentinel.
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
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

/// Ceiling on a single connect+authenticate+command round trip, so a wedged game
/// can't hang the control API handler indefinitely.
const RCON_TIMEOUT: Duration = Duration::from_secs(15);

/// Poll interval between RCON connect attempts while the game's listener is still
/// coming up. A freshly created or just-restarted server accepts players on the
/// game port slightly before its RCON port binds, so a command issued in that
/// window would otherwise hit a connection-refused; we retry across it instead.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// Cap on the reply text returned to the bot. Console replies are normally short;
/// this guards the Discord relay path against a pathological dump (e.g. `help`).
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// Random bytes behind the minted password; hex-encoded to twice this many chars.
const PASSWORD_BYTES: usize = 24;

/// Packet type ids from the Source RCON protocol. `EXECCOMMAND` and
/// `AUTH_RESPONSE` share the value 2 — they're disambiguated by the phase, not
/// the wire, so both constants exist for readability at their call sites.
const TYPE_RESPONSE_VALUE: i32 = 0;
const TYPE_EXECCOMMAND: i32 = 2;
const TYPE_AUTH_RESPONSE: i32 = 2;
const TYPE_AUTH: i32 = 3;

/// Request ids we stamp on our outgoing packets so replies can be correlated. The
/// server echoes the id back; an auth reply carrying `-1` means the password was
/// rejected.
const ID_AUTH: i32 = 1;
const ID_EXEC: i32 = 2;
const ID_SENTINEL: i32 = 3;
const ID_AUTH_FAILED: i32 = -1;

/// A packet is at minimum an id (4) + type (4) + two null terminators.
const MIN_PACKET_LEN: usize = 10;
/// Guard against a hostile length prefix forcing a huge allocation on read.
const MAX_PACKET_LEN: usize = 64 * 1024;
/// Source caps a request packet near 4 KiB; keep command bodies well under it.
const MAX_COMMAND_LEN: usize = 4000;

/// What the control layer needs to speak RCON to the local game: the loopback
/// port, the minted password, and whether to run in Minecraft-quirks mode.
pub struct RconRuntime {
    port: u16,
    password: String,
    minecraft_quirks: bool,
}

impl RconRuntime {
    /// Build a runtime for `port`, minting a fresh random password. When
    /// `max_password_len` is `Some(n)`, the minted password is truncated to `n`
    /// characters — for games that cap their RCON/admin password (Palworld's
    /// 30-char `ADMIN_PASSWORD`). The password is lowercase hex (ASCII), so
    /// truncating at any character index stays on a valid boundary, and the same
    /// truncated value is both injected into the game and used to authenticate.
    ///
    /// # Errors
    ///
    /// Returns an error if the system random source can't be read.
    pub fn new(port: u16, minecraft_quirks: bool, max_password_len: Option<usize>) -> Result<Self> {
        let mut password = generate_password()?;
        if let Some(max) = max_password_len
            && password.len() > max
        {
            password.truncate(max);
        }
        Ok(Self {
            port,
            password,
            minecraft_quirks,
        })
    }

    /// The minted password, for injecting into the game child's environment so the
    /// game configures its RCON server with the same value.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Run one console command over RCON and return the game's reply text
    /// (truncated to a guard size).
    ///
    /// # Errors
    ///
    /// Returns an error if the connection, authentication, or command fails or
    /// exceeds [`RCON_TIMEOUT`], so the control layer can surface it as an HTTP
    /// error rather than hang.
    pub async fn run_command(&self, command: &str) -> Result<String> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let output = timeout(RCON_TIMEOUT, self.exec(address, command))
            .await
            .with_context(|| {
                format!("rcon command timed out after {}s", RCON_TIMEOUT.as_secs())
            })??;
        Ok(truncate_output(output))
    }

    /// Broadcast `message` to everyone on the server, using the game's own
    /// broadcast command so the caller never has to know per-game console syntax.
    ///
    /// # Errors
    ///
    /// Returns an error if the console can't be reached or the command fails, same
    /// as [`Self::run_command`].
    pub async fn broadcast(&self, message: &str) -> Result<String> {
        self.run_command(&broadcast_command(message, self.minecraft_quirks)?)
            .await
    }

    /// Flush pending world state and pause further saves so a live snapshot is
    /// internally consistent, then the caller archives `/data`. A no-op for games
    /// not in Minecraft mode — `save-off`/`save-all` are Minecraft console verbs,
    /// so a generic RCON game gets an un-quiesced (still usable) snapshot rather
    /// than a spurious command. Pair every call with [`Self::resume_saves`].
    ///
    /// # Errors
    ///
    /// Returns an error if a save command can't be delivered to the console.
    pub async fn quiesce_for_snapshot(&self) -> Result<()> {
        if !self.minecraft_quirks {
            return Ok(());
        }
        self.run_command("save-off").await?;
        self.run_command("save-all flush").await?;
        Ok(())
    }

    /// Re-enable world saves after a snapshot. A no-op outside Minecraft mode.
    /// Always run this to undo [`Self::quiesce_for_snapshot`], even if the
    /// snapshot failed, so saves are never left disabled.
    ///
    /// # Errors
    ///
    /// Returns an error if the `save-on` command can't be delivered.
    pub async fn resume_saves(&self) -> Result<()> {
        if !self.minecraft_quirks {
            return Ok(());
        }
        self.run_command("save-on").await?;
        Ok(())
    }

    async fn exec(&self, address: SocketAddr, command: &str) -> Result<String> {
        if command.len() > MAX_COMMAND_LEN {
            bail!("command is too long for rcon ({} bytes)", command.len());
        }
        let mut stream = connect_with_retry(address).await?;
        authenticate(&mut stream, &self.password).await?;
        write_packet(&mut stream, ID_EXEC, TYPE_EXECCOMMAND, command).await?;
        if self.minecraft_quirks {
            // Minecraft replies with a single response packet; read just that.
            let packet = read_packet(&mut stream)
                .await
                .context("failed to read rcon command response")?;
            Ok(packet.body)
        } else {
            read_fragmented_response(&mut stream).await
        }
    }
}

/// Redacted so the minted password can never land in a `Debug` log line.
impl fmt::Debug for RconRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RconRuntime")
            .field("port", &self.port)
            .field("password", &"<redacted>")
            .field("minecraft_quirks", &self.minecraft_quirks)
            .finish()
    }
}

/// A decoded RCON packet: the echoed request id, the packet type, and the body
/// text (with its trailing null terminators stripped).
#[derive(Debug, PartialEq, Eq)]
struct Packet {
    id: i32,
    kind: i32,
    body: String,
}

/// Build the console command that broadcasts `message` to all players. Minecraft
/// uses `tellraw @a` with a JSON text component (built with `serde_json` so the
/// message is escaped, never hand-quoted); other RCON games fall back to a
/// Source-style `say`.
///
/// # Errors
///
/// Returns an error only if the message can't be encoded as JSON.
fn broadcast_command(message: &str, minecraft: bool) -> Result<String> {
    if minecraft {
        let component = serde_json::to_string(&serde_json::json!({
            "text": message,
            "color": "yellow",
        }))
        .context("failed to encode the tellraw message")?;
        Ok(format!("tellraw @a {component}"))
    } else {
        Ok(format!("say {message}"))
    }
}

/// Connect to the game's RCON port, retrying while the connection is refused —
/// the listener may not have bound yet on a server that just (re)started, even
/// though the game port is already accepting players. The caller's
/// [`RCON_TIMEOUT`] bounds the loop, so a genuinely down console eventually
/// surfaces as a timeout rather than looping forever. Any error other than
/// connection-refused is a real fault and fails immediately.
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

/// Read a possibly multi-packet Source reply. A sentinel `RESPONSE_VALUE` is sent
/// after the command; the server mirrors it back, so its echoed id marks the end
/// of the command's response packets.
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

/// Frame an RCON packet: little-endian length, id, type, the body, and two null
/// terminators. The length counts everything after itself.
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

/// Read one framed packet: the length prefix, then id, type, body, and the two
/// trailing null bytes.
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
    // Length includes the id and type (8 bytes) already read; the remainder is the
    // body followed by two null terminators.
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

/// Mint a random lowercase-hex password from the system CSPRNG.
///
/// # Errors
///
/// Returns an error if the system random source can't be read.
pub fn generate_password() -> Result<String> {
    let mut bytes = [0_u8; PASSWORD_BYTES];
    getrandom::fill(&mut bytes)
        .context("failed to read system random source for the rcon password")?;
    let mut password = String::with_capacity(PASSWORD_BYTES * 2);
    for byte in bytes {
        // Writing hex to a String is infallible; propagate rather than swallow to
        // satisfy the no-silent-error lints without an unwrap.
        write!(password, "{byte:02x}").context("failed to format the rcon password")?;
    }
    Ok(password)
}

/// Trim `output` to [`MAX_OUTPUT_BYTES`] at a UTF-8 boundary, flagging the cut.
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
