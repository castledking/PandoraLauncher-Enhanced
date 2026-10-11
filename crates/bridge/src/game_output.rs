#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameOutputLogLevel {
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Other,
}

impl GameOutputLogLevel {
    /// The level as it is shown and copied, kept in one place so the shaped column and
    /// the text put on the clipboard can't drift apart.
    pub fn name(self) -> &'static str {
        match self {
            GameOutputLogLevel::Fatal => "FATAL",
            GameOutputLogLevel::Error => "ERROR",
            GameOutputLogLevel::Warn => "WARN",
            GameOutputLogLevel::Info => "INFO",
            GameOutputLogLevel::Debug => "DEBUG",
            GameOutputLogLevel::Trace => "TRACE",
            GameOutputLogLevel::Other => "OTHER",
        }
    }
}
