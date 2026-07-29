use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("packet too short: {0} bytes")]
    TooShort(usize),
    #[error("length mismatch: header says {declared}, buffer has {actual}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("invalid command framing at offset {0}")]
    BadCommand(usize),
    #[error("command name is not valid ASCII")]
    BadCommandName,
}
