export interface TorrentFile {
    name: string;
    size: number;
    index: number;
}

export interface TorrentInfo {
    id?: string;
    name: string;
    total_size: number;
    files: TorrentFile[];
}

export interface DownloadItem {
    id: string;
    filename: string;
    url: string;
    size: number;
    downloaded: number;
    network_received?: number;
    verified_speed?: number;
    speed: number;
    eta: number;
    connections: number;
    protocol: "http" | "torrent";
    status: "downloading" | "paused" | "completed" | "queued" | "error";
    filepath: string;
    status_text?: string;
    status_phase?: string;
    phase_elapsed_secs?: number;
    error_message?: string | null;
    metadata: string | null;
    user_agent: string | null;
    cookies: string | null;
    category: string;
}

export interface ProgressPayload {
    id: string;
    total: number;
    downloaded: number;
    network_received?: number;
    verified_speed?: number;
    speed: number;
    eta: number;
    connections: number;
    status_text?: string;
    status_phase?: string;
    phase_elapsed_secs?: number;
}
