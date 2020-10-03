use std::{
    fmt,
    path::{Path, PathBuf},
};

use crate::{error::*, storage_info::FsStructure, FileInfo, Sha1Hash};

/// The parsed and validated torrent metainfo file, containing necessary
/// arguments for starting a torrent.
pub struct Metainfo {
    /// The name of the torrent, which is usually used to form the download
    /// path.
    pub name: String,
    /// This hash is used to identify a torrent with trackers and peers.
    pub info_hash: Sha1Hash,
    /// The concatenation of the 20 byte SHA-1 hash of each piece in torrent.
    /// This is used to verify the data sent to us by peers.
    pub pieces: Vec<u8>,
    /// The nominal lengths of a piece, that is, the length of all but
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
            if len == 0 {
                log::warn!("File length is 0");
                return Err(Error::InvalidMetainfo);
            }

            FsStructure::File(FileInfo {
                path: metainfo.info.name.clone().into(),
                len,
                torrent_offset: 0,
            })
        } else if let Some(files) = &metainfo.info.files {
            if files.is_empty() {
                log::warn!("Metainfo files must not be empty");
                return Err(Error::InvalidMetainfo);
            }

            // map the file information entries to our internal representation
            let mut file_infos = Vec::with_capacity(files.len());
            // and sum up the file offsets in the torrent
            let mut torrent_offset = 0;
            for file in files.into_iter() {
                // verify that the file length is non-zero
                if file.len == 0 {
                    log::warn!("File {:?} length is 0", file.path);
                    return Err(Error::InvalidMetainfo);
                }

                // verify that the path is not empty
                let path: PathBuf = file.path.iter().collect();
                if path == PathBuf::new() {
                    log::warn!("Path in metainfo is empty");
                    return Err(Error::InvalidMetainfo);
                }

                // verify that the path is not absolute
                if path.is_absolute() {
                    log::warn!("Path {:?} is absolute", path);
                    return Err(Error::InvalidMetainfo);
                }

                // verify that the path is not the root
                if path == Path::new("/") {
                    log::warn!("Path {:?} is root", path);
                    return Err(Error::InvalidMetainfo);
                }

                // file is now verified, we can collect it
                file_infos.push(FileInfo {
                    path,
                    torrent_offset,
                    len: file.len,
                });

                // advance offset for next file
                torrent_offset += file.len;
            }

            FsStructure::Archive { files: file_infos }
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

impl fmt::Debug for Metainfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metainfo")
            .field("name", &self.name)
            .field("info_hash", &self.info_hash)
            .field("pieces", &"<pieces...>")
            .field("piece_len", &self.piece_len)
            .field("structure", &self.structure)
            .finish()
    }
}

mod raw {
    //! Contains the types that we directly deserialize into, but is not to be used
    //! by the rest of the crate, as the validity of the parsed structure is not
    //! ensured at this level. The semantic validation happens in the [`Metainfo`]
    //! type, which is essentially a mapping of [`raw::Metainfo`], but with semantic
    //! requirements encoded in the type system..

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
