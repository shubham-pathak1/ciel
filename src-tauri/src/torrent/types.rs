use serde::{Deserialize, Serialize};

/// Summary of a single file within a torrent.
#[derive(Serialize, Deserialize, Clone)]
pub struct TorrentFile {
    pub name: String,
    pub size: u64,
    /// Internal index used by `librqbit` to identify the file.
    pub index: usize,
}

/// Consolidated metadata for a BitTorrent source.
#[derive(Serialize, Deserialize, Clone)]
pub struct TorrentInfo {
    /// Ephemeral analysis token used to avoid fetching metadata twice.
    pub id: Option<String>,
    pub name: String,
    pub total_size: u64,
    /// Flattened list of all files available in the torrent.
    pub files: Vec<TorrentFile>,
}
