//! Defines logging macros for use solely by `peer::PeerSession`.
//!
//! The first parameter has to be `self` of the `peer::PeerSession` instance,
//! while the rest of the parameters are the format string and its arguments.
//!
//! The macros prepend the log message with the peer's address. This makes the
//! log unique and allows filtering down messages to a single peer.

macro_rules! peer_warn {
    ($self:ident, $($arg:tt)*) => ({
        ::log::warn!("[{}] {}", $self.addr, format!($($arg)*));
    })
}

macro_rules! peer_info {
    ($self:ident, $($arg:tt)*) => ({
        ::log::info!("[{}] {}", $self.addr, format!($($arg)*));
    })
}

macro_rules! peer_debug {
    ($self:ident, $($arg:tt)*) => ({
        ::log::debug!("[{}] {}", $self.addr, format!($($arg)*));
    })
}

macro_rules! peer_trace {
    ($self:ident, $($arg:tt)*) => ({
        ::log::trace!("[{}] {}", $self.addr, format!($($arg)*));
    })
}
