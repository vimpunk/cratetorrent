//! This module exports types commonly used by applications as a convenience.

pub use crate::{
    alert::{Alert, AlertReceiver},
    conf::Conf,
    engine::{self, EngineHandle, Mode, TorrentParams},
    error::Error,
    metainfo::Metainfo,
    TorrentId,
};
// this is needed for `AlertReceiver::next`
pub use futures::stream::StreamExt;
