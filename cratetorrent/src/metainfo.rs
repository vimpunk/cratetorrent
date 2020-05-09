use crate::{error::*, FileInfo, Sha1Hash};

/// The parsed and validated torrent metainfo file, containing necessary
/// arguments for starting a torrent.
#[derive(Debug)]
pub struct Metainfo {
    /// The name of the torrent, which is usually used to form the download
    /// path.
    pub name: String,
    /// This hash is used to identify a torrent with trackers and peers.
    pub info_hash: Sha1Hash,
    /// The concatenation of the 20 byte SHA-1 hash of each piece in torrent.
    /// This is used to verify the data sent to us by peers.
    pub pieces: Vec<u8>,
    /// The nominal lenght of a piece, that is, the length of all but
    /// potentially the last piece, which may be smaller.
    pub piece_len: u32,
    /// The paths and lenths of the downloaded files.
    pub structure: FsStructure,
}

impl Metainfo {
    /// Parses from a byte buffer a new [`Metainfo`] instance, or aborts with an
    /// error.
    ///
    /// If the encoding itself is correct, the constructor may still fail if the
    /// metadata is not semantically correct (e.g. if the length of the `pieces`
    /// field is not a multiple of 20, or no valid files are encoded, etc).
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        // parse metainfo, but correctly parsing is not enough, we need to
        // verify it afterwards
        let metainfo: raw::Metainfo = serde_bencode::from_bytes(buf)?;

        // the pieces field is a concatenation of 20 byte SHA-1 hashes, so it
        // must be a multiple of 20
        if metainfo.info.pieces.len() % 20 != 0 {
            return Err(Error::InvalidPieces);
        }

        // verify download structure
        let structure = if let Some(len) = metainfo.info.len {
            if metainfo.info.files.is_some() {
                log::warn!("Metainfo cannot contain both `length` and `files`");
                return Err(Error::InvalidMetainfo);
            }
            FsStructure::File { len }
        } else if let Some(files) = &metainfo.info.files {
            if files.is_empty() {
                log::warn!("Metainfo files must not be empty");
                return Err(Error::InvalidMetainfo);
            }

            let files = files
                .iter()
                .map(|f| FileInfo {
                    path: f.path.iter().collect(),
                    len: f.len,
                })
                .collect();

            FsStructure::Archive { files }
        } else {
            log::warn!("No `length` or `files` key present in metainfo");
            return Err(Error::InvalidMetainfo);
        };

        // create info hash as a last step
        let info_hash = metainfo.create_info_hash()?;

        Ok(Self {
            name: metainfo.info.name,
            info_hash,
            pieces: metainfo.info.pieces,
            piece_len: metainfo.info.piece_len,
            structure,
        })
    }

    /// Returns the number of pieces in this torrent.
    pub fn piece_count(&self) -> usize {
        self.pieces.len() / 20
    }
}

/// Defines the file system structure of the download.
#[derive(Clone, Debug)]
pub enum FsStructure {
    File { len: u64 },
    Archive { files: Vec<FileInfo> },
}

impl FsStructure {
    /// Returns the total download size in bytes.
    ///
    /// Note that this is an O(n) operation for archive downloads, where n is
    /// the number of files, so this value should ideally be cached.
    pub fn download_len(&self) -> u64 {
        match self {
            Self::File { len } => *len,
            Self::Archive { files } => files.iter().map(|f| f.len).sum(),
        }
    }
}

/// Contains the types that we directly deserialize into, but is not to be used
/// by the rest of the crate, as the validity of the parsed structure is not
/// ensured at this level. The semantic validation happens in the [`Metainfo`]
/// type, which is essentially a mapping of [`raw::Metainfo`], but with semantic
/// requirements encoded in the type system..
mod raw {
    use sha1::{Digest, Sha1};

    use super::{Result, Sha1Hash};

    #[derive(Debug, Deserialize)]
    pub struct Metainfo {
        pub info: Info,
    }

    impl Metainfo {
        /// Creates a SHA-1 hash of the encoded `info` field's value.
        pub fn create_info_hash(&self) -> Result<Sha1Hash> {
            let info = serde_bencode::to_bytes(&self.info)?;
            let digest = Sha1::digest(&info);
            let mut info_hash = [0; 20];
            info_hash.copy_from_slice(&digest);
            Ok(info_hash)
        }
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct Info {
        pub name: String,
        #[serde(with = "serde_bytes")]
        pub pieces: Vec<u8>,
        #[serde(rename = "piece length")]
        pub piece_len: u32,
        #[serde(rename = "length")]
        pub len: Option<u64>,
        pub files: Option<Vec<File>>,
        /// This is not currently used but needs to be kept in here so that we
        /// can encode back a valid info hash for hashing.
        pub private: Option<u8>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct File {
        pub path: Vec<String>,
        #[serde(rename = "length")]
        pub len: u64,
    }
}

// TODO(https://github.com/mandreyel/cratetorrent/issues/8): add metainfo
// parsing tests
