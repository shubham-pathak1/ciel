//! Ciel Database Module
//! 
//! This module handles the persistence layer of the application using SQLite.
//! It manages the lifecycle of:
//! - **Downloads**: Metadata, status, and progress for all download types (HTTP, Torrent, Video).
//! - **Settings**: A flexible key-value store for application configuration.
//! - **Chunks**: Segment metadata used for resuming multi-connection HTTP downloads.
//! - **History**: An event log for auditing download activities (creation, errors, completion).

use rusqlite::{Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Shared state holding the absolute path to the SQLite database file.
pub struct DbState {
    pub path: String,
}

/// Centralized database accessor with a busy timeout to prevent contention hangs.
pub fn open_db<P: AsRef<Path>>(path: P) -> SqliteResult<Connection> {
    let conn = Connection::open(path)?;
    // Wait up to 5 seconds if the database is locked by another thread.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // Enable Foreign Keys to support ON DELETE CASCADE
    let _ = conn.execute("PRAGMA foreign_keys = ON;", []);
    // Enable WAL mode to allow concurrent reads and writes
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    // NORMAL synchronous mode is safe with WAL and much faster for sequential updates
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    Ok(conn)
}

/// Represents the current lifecycle stage of a download.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    /// Download is in the queue, waiting for resources.
    Queued,
    /// Data is actively being transferred.
    Downloading,
    /// User or system has temporarily halted the transfer.
    Paused,
    /// Transfer successfully finished and verified.
    Completed,
    /// An unrecoverable error occurred during transfer.
    Error,
}

impl DownloadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::Queued => "queued",
            DownloadStatus::Downloading => "downloading",
            DownloadStatus::Paused => "paused",
            DownloadStatus::Completed => "completed",
            DownloadStatus::Error => "error",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "queued" => DownloadStatus::Queued,
            "downloading" => DownloadStatus::Downloading,
            "paused" => DownloadStatus::Paused,
            "completed" => DownloadStatus::Completed,
            "error" => DownloadStatus::Error,
            _ => DownloadStatus::Queued,
        }
    }
}

/// Categorizes the download by its source protocol or content type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadProtocol {
    /// Standard web download (Direct Link).
    Http,
    /// Peer-to-peer download (Magnet/Torrent).
    Torrent,
    /// Extracted media stream (YouTube, etc.).
    Video,
}

impl DownloadProtocol {
    /// Serializes the enum to a string for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadProtocol::Http => "http",
            DownloadProtocol::Torrent => "torrent",
            DownloadProtocol::Video => "video",
        }
    }

    /// Deserializes a string from the database back into the enum.
    pub fn from_str(s: &str) -> Self {
        match s {
            "torrent" => DownloadProtocol::Torrent,
            "video" => DownloadProtocol::Video,
            _ => DownloadProtocol::Http,
        }
    }
}

/// The primary data structure representing a download record in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    /// Unique UUID (v4) string.
    pub id: String,
    /// Source URL (Direct Link, Magnet, or Page URL).
    pub url: String,
    /// Display name of the file.
    pub filename: String,
    /// Absolute target path on the local filesystem.
    pub filepath: String,
    /// Total expected size in bytes.
    pub size: i64,
    /// Bytes successfully written to disk.
    pub downloaded: i64,
    pub status: DownloadStatus,
    pub protocol: DownloadProtocol,
    /// Instantaneous transfer speed in bytes/sec.
    pub speed: i64,
    /// Number of active network connections.
    pub connections: i32,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// ISO 8601 completion timestamp.
    pub completed_at: Option<String>,
    /// Human-readable error details if status is Error.
    pub error_message: Option<String>,
    /// BitTorrent Info Hash (Hex string).
    pub info_hash: Option<String>,
    /// Extracted video/torrent metadata (JSON string).
    pub metadata: Option<String>,
    /// Custom User-Agent used for the request.
    pub user_agent: Option<String>,
    /// Request cookies for authenticated downloads.
    pub cookies: Option<String>,
    /// Organizational category (Movies, Music, etc.).
    pub category: String,
}

/// Bootstraps the SQLite database, creates tables, and applies schema migrations.
/// 
/// This is called once during application startup in `lib.rs`.
pub fn init_db<P: AsRef<Path>>(path: P) -> SqliteResult<()> {
    let conn = open_db(path)?;

    conn.execute_batch(
        "
        -- Downloads table
        CREATE TABLE IF NOT EXISTS downloads (
            id TEXT PRIMARY KEY,
            url TEXT NOT NULL,
            filename TEXT NOT NULL,
            filepath TEXT NOT NULL,
            size INTEGER NOT NULL DEFAULT 0,
            downloaded INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'queued',
            protocol TEXT NOT NULL DEFAULT 'http',
            speed INTEGER NOT NULL DEFAULT 0,
            connections INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            completed_at TEXT,
            error_message TEXT,
            info_hash TEXT,
            metadata TEXT,
            user_agent TEXT,
            cookies TEXT,
            category TEXT NOT NULL DEFAULT 'Other'
        );
        "
    )?;

    // Migrations (old user_agent and cookies migrations are now part of initial table creation)

    conn.execute_batch(
        "
        -- Settings table (key-value store)
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- Download chunks for resume support
        CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            download_id TEXT NOT NULL,
            start_byte INTEGER NOT NULL,
            end_byte INTEGER NOT NULL,
            downloaded INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'pending',
            FOREIGN KEY (download_id) REFERENCES downloads(id) ON DELETE CASCADE
        );

        -- History/events table
        CREATE TABLE IF NOT EXISTS history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            download_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            details TEXT,
            FOREIGN KEY (download_id) REFERENCES downloads(id) ON DELETE CASCADE
        );

        -- Indexes for performance
        CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
        CREATE INDEX IF NOT EXISTS idx_downloads_created ON downloads(created_at);
        CREATE INDEX IF NOT EXISTS idx_chunks_download ON chunks(download_id);
        CREATE INDEX IF NOT EXISTS idx_history_download ON history(download_id);

        -- Insert default settings if not exists
        INSERT OR IGNORE INTO settings (key, value) VALUES
            ('download_path', ''),
            ('max_concurrent', '3'),
            ('max_connections', '8'),
            ('auto_start', 'true'),
            ('notifications', 'true'),
            ('speed_limit', '0'),
            ('autocatch_enabled', 'true'),
            ('torrent_encryption', 'false'),
            ('open_folder_on_finish', 'false'),
            ('shutdown_on_finish', 'false'),
            ('sound_on_finish', 'false'),
            ('theme', 'dark'),
            ('scheduler_enabled', 'false'),
            ('scheduler_start_time', '02:00'),
            ('scheduler_pause_time', '08:00'),
            ('category_filter', 'All'),
            ('max_retries', '5'),
            ('retry_delay', '5'),
            ('cookie_browser', 'none'),
            ('ask_location', 'false'),
            ('auto_organize', 'false');
        "
    )?;

    // Migration: Add metadata column to downloads table if it doesn't exist
    {
        let mut stmt = conn.prepare("PRAGMA table_info(downloads)")?;
        let columns = stmt.query_map([], |row| {
            let name: String = row.get(1)?;
            Ok(name)
        })?;

        let mut has_metadata = false;
        for col in columns {
            if let Ok(name) = col {
                if name == "metadata" {
                    has_metadata = true;
                    break;
                }
            }
        }

        if !has_metadata {
            conn.execute("ALTER TABLE downloads ADD COLUMN metadata TEXT ", [])?;
        }
    }

    // Migration: Add category column to downloads table if it doesn't exist
    {
        let mut stmt = conn.prepare("PRAGMA table_info(downloads)")?;
        let columns = stmt.query_map([], |row| {
            let name: String = row.get(1)?;
            Ok(name)
        })?;

        let mut has_category = false;
        for col in columns {
            if let Ok(name) = col {
                if name == "category" {
                    has_category = true;
                    break;
                }
            }
        }

        if !has_category {
            conn.execute("ALTER TABLE downloads ADD COLUMN category TEXT NOT NULL DEFAULT 'Other'", [])?;
        }
    }

    Ok(())
}

/// Maps a database row to a `Download` struct.
/// Internal helper used to DRY up mapping logic.
fn row_to_download(row: &rusqlite::Row) -> SqliteResult<Download> {
    Ok(Download {
        id: row.get(0)?,
        url: row.get(1)?,
        filename: row.get(2)?,
        filepath: row.get(3)?,
        size: row.get(4)?,
        downloaded: row.get(5)?,
        status: DownloadStatus::from_str(&row.get::<_, String>(6)?),
        protocol: DownloadProtocol::from_str(&row.get::<_, String>(7)?),
        speed: row.get(8)?,
        connections: row.get(9)?,
        created_at: row.get(10)?,
        completed_at: row.get(11)?,
        error_message: row.get(12)?,
        info_hash: row.get(13)?,
        metadata: row.get(14)?,
        user_agent: row.get(15)?,
        cookies: row.get(16)?,
        category: row.get(17)?,
    })
}

/// Retrieves all download records from the database, sorted by creation date (newest first).
pub fn get_all_downloads<P: AsRef<Path>>(db_path: P) -> SqliteResult<Vec<Download>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, url, filename, filepath, size, downloaded, status, protocol, speed, connections, created_at, completed_at, error_message, info_hash, metadata, user_agent, cookies, category
         FROM downloads
         ORDER BY created_at DESC "
    )?;

    let downloads = stmt
        .query_map([], |row| row_to_download(row))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(downloads)
}

/// Retrieves all downloads that have successfully reached the 'completed' status.
pub fn get_history<P: AsRef<Path>>(db_path: P) -> SqliteResult<Vec<Download>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, url, filename, filepath, size, downloaded, status, protocol, speed, connections, created_at, completed_at, error_message, info_hash, metadata, user_agent, cookies, category
         FROM downloads
         WHERE status = 'completed'
         ORDER BY completed_at DESC "
    )?;

    let downloads = stmt
        .query_map([], |row| row_to_download(row))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(downloads)
}

/// Persists a new download record to the database.
pub fn insert_download<P: AsRef<Path>>(db_path: P, download: &Download) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "INSERT INTO downloads (id, url, filename, filepath, size, downloaded, status, protocol, speed, connections, created_at, completed_at, error_message, info_hash, metadata, user_agent, cookies, category)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        rusqlite::params![
            &download.id,
            &download.url,
            &download.filename,
            &download.filepath,
            download.size,
            download.downloaded,
            download.status.as_str(),
            download.protocol.as_str(),
            download.speed,
            download.connections,
            &download.created_at,
            &download.completed_at,
            &download.error_message,
            &download.info_hash,
            &download.metadata,
            &download.user_agent,
            &download.cookies,
            &download.category,
        ],
    )?;
    Ok(())
}

/// Retrieves the status of a specific download.
pub fn get_download_status(conn: &Connection, id: &str) -> SqliteResult<DownloadStatus> {
    let status_str: String = conn.query_row(
        "SELECT status FROM downloads WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    Ok(DownloadStatus::from_str(&status_str))
}

/// Updates the status field (e.g., from 'Downloading' to 'Paused') for a specific record.
pub fn update_download_status<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    status: DownloadStatus,
) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "UPDATE downloads SET status = ?1 WHERE id = ?2",
        (status.as_str(), id),
    )?;
    Ok(())
}

/// Periodically called to update the current byte count and transfer speed.
pub fn update_download_progress<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    downloaded: i64,
    speed: i64,
) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "UPDATE downloads SET downloaded = ?1, speed = ?2 WHERE id = ?3",
        (downloaded, speed, id),
    )?;
    Ok(())
}

/// Updates the total size of a download. Useful when size is determined after metadata extraction.
pub fn update_download_size<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    size: i64,
) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "UPDATE downloads SET size = ?1 WHERE id = ?2",
        (size, id),
    )?;
    Ok(())
}

/// Updates the filename of a download.
pub fn update_download_name<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    name: &str,
) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "UPDATE downloads SET filename = ?1 WHERE id = ?2",
        (name, id),
    )?;
    Ok(())
}

pub fn update_download_cookies<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    cookies: &str,
) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "UPDATE downloads SET cookies = ?1 WHERE id = ?2",
        (cookies, id),
    )?;
    Ok(())
}

/// Removes a download record and its associated chunks/history from the database.
pub fn delete_download_by_id<P: AsRef<Path>>(db_path: P, id: &str) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute("DELETE FROM downloads WHERE id = ?1", [id])?;
    Ok(())
}

/// Retrieves a configuration value by its unique key. Returns `None` if not found.
pub fn get_setting<P: AsRef<Path>>(db_path: P, key: &str) -> SqliteResult<Option<String>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let result = stmt.query_row([key], |row| row.get(0)).ok();
    Ok(result)
}

/// Stores or updates a configuration value in the `settings` table.
pub fn set_setting<P: AsRef<Path>>(db_path: P, key: &str, value: &str) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        (key, value),
    )?;
    Ok(())
}

/// Searches for an existing download by its source URL.
/// Used to prevent redundant transfers or to resume existing ones.
pub fn find_download_by_url<P: AsRef<Path>>(db_path: P, url: &str) -> SqliteResult<Option<Download>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare("SELECT id, url, filename, filepath, size, downloaded, status, protocol, speed, connections, created_at, completed_at, error_message, info_hash, metadata, user_agent, cookies, category FROM downloads WHERE url = ?1")?;
    
    let mut rows = stmt.query([url])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_download(row)?))
    } else {
        Ok(None)
    }
}

pub fn check_filepath_exists<P: AsRef<Path>>(db_path: P, filepath: &str) -> SqliteResult<bool> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare("SELECT COUNT(*) FROM downloads WHERE filepath = ?1")?;
    let count: i64 = stmt.query_row([filepath], |row| row.get(0))?;
    Ok(count > 0)
}

/// Stores a batch of chunk metadata for multi-threaded HTTP downloads.
pub fn insert_chunks<P: AsRef<Path>>(db_path: P, chunks: Vec<crate::downloader::ChunkRecord>) -> SqliteResult<()> {
    let mut conn = open_db(db_path)?;
    let tx = conn.transaction()?;
    {
        for chunk in chunks {
            tx.execute(
                "INSERT INTO chunks (download_id, start_byte, end_byte, downloaded, status) VALUES (?1, ?2, ?3, ?4, ?5)",
                (&chunk.download_id, chunk.start, chunk.end, chunk.downloaded, "pending"),
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn update_chunk_progress<P: AsRef<Path>>(db_path: P, download_id: &str, start_byte: i64, downloaded: i64) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "UPDATE chunks SET downloaded = ?1 WHERE download_id = ?2 AND start_byte = ?3",
        (downloaded, download_id, start_byte),
    )?;
    Ok(())
}

pub fn get_download_chunks<P: AsRef<Path>>(db_path: P, download_id: &str) -> SqliteResult<Vec<crate::downloader::ChunkRecord>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare("SELECT start_byte, end_byte, downloaded FROM chunks WHERE download_id = ?1")?;
    let chunks = stmt.query_map([download_id], |row| {
        Ok(crate::downloader::ChunkRecord {
            download_id: download_id.to_string(),
            start: row.get(0)?,
            end: row.get(1)?,
            downloaded: row.get(2)?,
        })
    })?.collect::<Result<Vec<_>, _>>()?;
    Ok(chunks)
}


/// Get all settings as key-value pairs
pub fn get_all_settings<P: AsRef<Path>>(
    db_path: P,
) -> SqliteResult<std::collections::HashMap<String, String>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare("SELECT key, value FROM settings ")?;
    let settings = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
    Ok(settings)
}

/// Log a download event
pub fn log_event<P: AsRef<Path>>(
    db_path: P,
    download_id: &str,
    event_type: &str,
    details: Option<&str>,
) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO history (download_id, event_type, timestamp, details) VALUES (?1, ?2, ?3, ?4)",
        (download_id, event_type, now, details),
    )?;
    Ok(())
}

/// Get events for a download
pub fn get_download_events<P: AsRef<Path>>(
    db_path: P,
    download_id: &str,
) -> SqliteResult<Vec<(String, String, Option<String>)>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT event_type, timestamp, details FROM history WHERE download_id = ?1 ORDER BY timestamp DESC "
    )?;

    let events = stmt
        .query_map([download_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(events)
}

/// Delete all finished (completed or error) downloads
pub fn delete_finished_downloads<P: AsRef<Path>>(db_path: P) -> SqliteResult<()> {
    let conn = open_db(db_path)?;
    conn.execute(
        "DELETE FROM downloads WHERE status = 'completed' OR status = 'error'",
        [],
    )?;
    
    // Also cleanup related chunks and history
    let _ = conn.execute("DELETE FROM chunks WHERE download_id NOT IN (SELECT id FROM downloads)", []);
    let _ = conn.execute("DELETE FROM history WHERE download_id NOT IN (SELECT id FROM downloads)", []);
    
    Ok(())
}
