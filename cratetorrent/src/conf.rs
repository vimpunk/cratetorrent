use std::path::PathBuf;

use crate::PeerId;

#[derive(Clone, Debug)]
pub struct Conf {
    pub engine: EngineConf,
    pub torrent: TorrentConf,
}

#[derive(Clone, Debug)]
pub struct EngineConf {
    pub client_id: PeerId,
}

#[derive(Clone, Debug)]
pub struct TorrentConf {
    pub download_dir: PathBuf,
}
