use crate::error::*;
use crate::Sha1Hash;
use sha1::{Digest, Sha1};

#[derive(Debug, Deserialize)]
pub struct Metainfo {
    pub info: Info,
}

impl Metainfo {
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        let metainfo: Self = serde_bencode::from_bytes(buf)?;
        // the pieces field is a concatenation of 20 byte SHA-1 hashes, so it
        // must be a multiple of 20
        if metainfo.info.pieces.len() % 20 != 0 {
            return Err(Error::InvalidPieces);
        }
        Ok(metainfo)
    }

    pub fn piece_count(&self) -> usize {
        self.info.pieces.len() / 20
    }

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
    pub piece_length: u64,
    pub length: Option<u64>,
    pub files: Option<Vec<File>>,
    pub private: Option<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct File {
    pub path: Vec<String>,
    pub length: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: add metainfo parsing tests
}
