use crate::error::ProtocolError;

/// Initialization-complete command name.
pub const INIT_COMPLETE: CommandName = CommandName(*b"InCm");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandName(pub [u8; 4]);

impl CommandName {
    pub fn as_str(&self) -> Result<&str, ProtocolError> {
        std::str::from_utf8(&self.0).map_err(|_| ProtocolError::BadCommandName)
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, ProtocolError> {
        if b.len() < 4 {
            return Err(ProtocolError::BadCommandName);
        }
        let mut name = [0u8; 4];
        name.copy_from_slice(&b[..4]);
        Ok(Self(name))
    }
}

impl std::fmt::Display for CommandName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.as_str() {
            Ok(s) => write!(f, "{s}"),
            Err(_) => write!(f, "{:?}", self.0),
        }
    }
}

/// A single framed ATEM command inside a packet payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRef<'a> {
    pub name: CommandName,
    /// Full framed command including 8-byte header (length + reserved + name) + body.
    pub raw: &'a [u8],
    pub body: &'a [u8],
}

/// Serialize a command from name + body into wire bytes.
pub fn serialize_command(name: CommandName, body: &[u8]) -> Vec<u8> {
    let len = 8 + body.len();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&name.0);
    out.extend_from_slice(body);
    out
}

/// Parse zero or more commands from a packet payload.
pub fn parse_commands(payload: &[u8]) -> Result<Vec<CommandRef<'_>>, ProtocolError> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + 8 <= payload.len() {
        let len = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
        if len < 8 || offset + len > payload.len() {
            return Err(ProtocolError::BadCommand(offset));
        }
        let name = CommandName::from_bytes(&payload[offset + 4..offset + 8])?;
        let raw = &payload[offset..offset + len];
        let body = &payload[offset + 8..offset + len];
        out.push(CommandRef { name, raw, body });
        offset += len;
    }
    if offset != payload.len() {
        return Err(ProtocolError::BadCommand(offset));
    }
    Ok(out)
}

/// Parse `_ver` body: major/minor as two big-endian u16s when present.
pub fn parse_version(body: &[u8]) -> Option<(u16, u16)> {
    if body.len() < 4 {
        return None;
    }
    Some((
        u16::from_be_bytes([body[0], body[1]]),
        u16::from_be_bytes([body[2], body[3]]),
    ))
}

/// Build a synthetic `InCm` for late-join dumps.
///
/// Real ATEM / LibAtem emit a 4-byte body (`01 00 00 00`), i.e. a 12-byte command.
/// Companion's `atem-connection` parser uses `while (buffer.length > 8)`, so an
/// empty 8-byte `InCm` left as the trailing remainder of a packet is never
/// deserialized — the client spins on "Connecting" forever.
pub fn synthetic_init_complete() -> Vec<u8> {
    serialize_command(INIT_COMPLETE, &[0x01, 0x00, 0x00, 0x00])
}

/// Identity bytes used for state-cache coalescing.
///
/// Prefer a short stable prefix of the body (indexes), falling back to the full body
/// for small commands so unknown opcodes still coalesce by content.
pub fn command_identity(name: CommandName, body: &[u8]) -> Vec<u8> {
    match &name.0 {
        b"_ver" | b"InCm" | b"_pin" | b"_top" | b"powr" => Vec::new(),
        // Program/preview: coalesce per ME (byte 0), not per source value.
        b"PrgI" | b"PrvI" | b"TrSS" | b"TrPs" | b"TrPr" | b"FtbS" | b"FtbP" => {
            body.first().copied().into_iter().collect()
        }
        // Indexed state: first few bytes encode ME/keyer/aux index.
        b"KeOn" | b"KeBP" | b"DskB" | b"DskS" | b"DskP" | b"ColV" | b"AuxS" | b"MPCE" | b"MPrp"
        | b"CvlI" | b"InPr" | b"InPX" => body.get(..4).unwrap_or(body).to_vec(),
        _ => {
            if body.len() <= 64 {
                body.to_vec()
            } else {
                // Large opaque: coalesce by name only (latest wins for that opcode).
                Vec::new()
            }
        }
    }
}

/// High-rate / non-state commands that must not enter the late-join cache.
pub fn is_ephemeral_command(name: CommandName) -> bool {
    matches!(
        &name.0,
        b"AMLv" | b"AMmO" | b"FASP" | b"FMLv" | b"FAtl" | b"FAsp"
    )
}

pub fn is_lock_command(name: CommandName) -> bool {
    matches!(&name.0, b"LOCK" | b"PLCK" | b"LKST" | b"LKOB")
        || name.as_str().ok().is_some_and(|s| s.starts_with("Lock"))
}

/// Client-originated lock request (not upstream status).
pub fn is_lock_request(name: CommandName) -> bool {
    matches!(&name.0, b"LOCK" | b"PLCK")
}

/// Upstream lock status/obtainment notifications.
pub fn is_lock_status(name: CommandName) -> bool {
    matches!(&name.0, b"LKST" | b"LKOB")
}

/// Best-effort store id from lock/transfer command body (u16 BE at offset 0).
pub fn lock_store_id(body: &[u8]) -> Option<u16> {
    if body.len() < 2 {
        return None;
    }
    Some(u16::from_be_bytes([body[0], body[1]]))
}

/// Whether a LOCK/PLCK body requests locked (true) vs unlocked (false).
pub fn lock_request_enabled(body: &[u8]) -> bool {
    // Common layouts: last byte or byte[2] is boolean state.
    if body.len() >= 3 {
        return body[2] != 0;
    }
    body.last().copied().unwrap_or(0) != 0
}

/// Build a client unlock request for a store (LOCK store, state=0).
pub fn synthesize_unlock(store: u16) -> Vec<u8> {
    let mut body = Vec::with_capacity(4);
    body.extend_from_slice(&store.to_be_bytes());
    body.push(0);
    body.push(0);
    serialize_command(CommandName(*b"LOCK"), &body)
}

pub fn is_transfer_command(name: CommandName) -> bool {
    matches!(
        &name.0,
        b"FTSD" | b"FTSU" | b"FTCD" | b"FTDa" | b"FTUA" | b"FTFD" | b"FTDC" | b"FTDE" | b"FTES"
    ) || name
        .as_str()
        .ok()
        .is_some_and(|s| s.starts_with("FT") || s.starts_with("Data"))
}

/// Audio level subscription related client commands (LibAtem SendLevels family).
pub fn is_audio_levels_subscribe(name: CommandName) -> bool {
    matches!(&name.0, b"SALN" | b"FASP" | b"SAff")
        || name.as_str().ok().is_some_and(|s| {
            s.contains("SendLevels") || s.contains("Levels") && s.contains("Enable")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrip() {
        let raw = serialize_command(CommandName(*b"PrgI"), &[0, 0, 0, 1]);
        let cmds = parse_commands(&raw).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].name, CommandName(*b"PrgI"));
        assert_eq!(cmds[0].body, &[0, 0, 0, 1]);
    }

    #[test]
    fn identity_coalesces_same_me() {
        let a = command_identity(CommandName(*b"PrgI"), &[0, 0, 0, 1]);
        let b = command_identity(CommandName(*b"PrgI"), &[0, 0, 0, 2]);
        assert_eq!(a, b);
        let other_me = command_identity(CommandName(*b"PrgI"), &[1, 0, 0, 2]);
        assert_ne!(a, other_me);
    }

    #[test]
    fn synthetic_incm_is_twelve_bytes_like_libatem() {
        let raw = synthetic_init_complete();
        assert_eq!(raw.len(), 12, "Companion skips trailing 8-byte InCm");
        let cmds = parse_commands(&raw).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].name, INIT_COMPLETE);
        assert_eq!(cmds[0].body, &[0x01, 0x00, 0x00, 0x00]);
    }
}
