//! This module exports types commonly used by applications as a convenience.

pub use crate::{
    alert::Alert,
    conf::Conf,
    engine::{self, Mode, TorrentParams},
    error::Error,
    metainfo::Metainfo,
};
// this is needed for `AlertReceiver::next`
pub use futures::stream::StreamExt;
