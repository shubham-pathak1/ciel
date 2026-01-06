//! Database module for Ciel
//!
//! Handles all SQLite operations including:
//! - Database initialization and migrations
//! - Download CRUD operations
//! - Settings storage

use rusqlite::{Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Application database state
pub struct DbState {
    pub path: String,
}

/// Download status enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
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

/// Download protocol type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadProtocol {
    Http,
    Torrent,
    Video,
}

impl DownloadProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadProtocol::Http => "http",
            DownloadProtocol::Torrent => "torrent",
            DownloadProtocol::Video => "video",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "torrent" => DownloadProtocol::Torrent,
            "video" => DownloadProtocol::Video,
            _ => DownloadProtocol::Http,
        }
    }
}

/// Download record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub filepath: String,
    pub size: i64,
    pub downloaded: i64,
    pub status: DownloadStatus,
    pub protocol: DownloadProtocol,
    pub speed: i64,
    pub connections: i32,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
    pub info_hash: Option<String>,
    pub metadata: Option<String>,
}

/// Initialize the database with schema
pub fn init_db<P: AsRef<Path>>(path: P) -> SqliteResult<()> {
    let conn = Connection::open(path)?;

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
            metadata TEXT
        );

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
            ('theme', 'dark');
        ",
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
            conn.execute("ALTER TABLE downloads ADD COLUMN metadata TEXT", [])?;
        }
    }

    Ok(())
}

/// Get all downloads from database
pub fn get_all_downloads<P: AsRef<Path>>(db_path: P) -> SqliteResult<Vec<Download>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, url, filename, filepath, size, downloaded, status, protocol, speed, connections, created_at, completed_at, error_message, info_hash, metadata 
         FROM downloads 
         ORDER BY created_at DESC",
    )?;

    let downloads = stmt
        .query_map([], |row| {
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
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(downloads)
}

/// Get completed downloads (History)
pub fn get_history<P: AsRef<Path>>(db_path: P) -> SqliteResult<Vec<Download>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, url, filename, filepath, size, downloaded, status, protocol, speed, connections, created_at, completed_at, error_message, info_hash, metadata 
         FROM downloads 
         WHERE status = 'completed'
         ORDER BY completed_at DESC",
    )?;

    let downloads = stmt
        .query_map([], |row| {
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
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(downloads)
}

/// Insert a new download
pub fn insert_download<P: AsRef<Path>>(db_path: P, download: &Download) -> SqliteResult<()> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "INSERT INTO downloads (id, url, filename, filepath, size, downloaded, status, protocol, speed, connections, created_at, completed_at, error_message, info_hash, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        (
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
        ),
    )?;
    Ok(())
}

/// Update download status
pub fn update_download_status<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    status: DownloadStatus,
) -> SqliteResult<()> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "UPDATE downloads SET status = ?1 WHERE id = ?2",
        (status.as_str(), id),
    )?;
    Ok(())
}

/// Update download progress
pub fn update_download_progress<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    downloaded: i64,
    speed: i64,
) -> SqliteResult<()> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "UPDATE downloads SET downloaded = ?1, speed = ?2 WHERE id = ?3",
        (downloaded, speed, id),
    )?;
    Ok(())
}

/// Update download size
pub fn update_download_size<P: AsRef<Path>>(
    db_path: P,
    id: &str,
    size: i64,
) -> SqliteResult<()> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "UPDATE downloads SET size = ?1 WHERE id = ?2",
        (size, id),
    )?;
    Ok(())
}

/// Delete a download
pub fn delete_download_by_id<P: AsRef<Path>>(db_path: P, id: &str) -> SqliteResult<()> {
    let conn = Connection::open(db_path)?;
    conn.execute("DELETE FROM downloads WHERE id = ?1", [id])?;
    Ok(())
}

/// Get a setting value
pub fn get_setting<P: AsRef<Path>>(db_path: P, key: &str) -> SqliteResult<Option<String>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let result = stmt.query_row([key], |row| row.get(0)).ok();
    Ok(result)
}

/// Update a setting
pub fn set_setting<P: AsRef<Path>>(db_path: P, key: &str, value: &str) -> SqliteResult<()> {
    let conn = Connection::open(db_path)?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        (key, value),
    )?;
    Ok(())
}

/// Chunks management
pub fn insert_chunks<P: AsRef<Path>>(db_path: P, chunks: Vec<crate::downloader::ChunkRecord>) -> SqliteResult<()> {
    let mut conn = Connection::open(db_path)?;
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
    let conn = Connection::open(db_path)?;
    conn.execute(
        "UPDATE chunks SET downloaded = ?1 WHERE download_id = ?2 AND start_byte = ?3",
        (downloaded, download_id, start_byte),
    )?;
    Ok(())
}

pub fn get_download_chunks<P: AsRef<Path>>(db_path: P, download_id: &str) -> SqliteResult<Vec<crate::downloader::ChunkRecord>> {
    let conn = Connection::open(db_path)?;
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
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
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
    let conn = Connection::open(db_path)?;
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
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT event_type, timestamp, details FROM history WHERE download_id = ?1 ORDER BY timestamp DESC",
    )?;

    let events = stmt
        .query_map([download_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(events)
}
