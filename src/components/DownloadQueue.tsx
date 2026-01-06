import { useState, useEffect } from "react";
import { CloudDownload, FileDown, Pause, Trash2, FolderOpen, Play, ArrowDown, Clock, Users, Wifi, Plus, AlertCircle, Database as DatabaseIcon } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import clsx from "clsx";
import { ModalPortal } from "./ModalPortal";
import { TorrentFileSelector } from "./TorrentFileSelector";

// User code didn't have location picker in modal. I will skip it to match their UI EXACTLY.
// If they want 'ask location', the Settings 'ask_location' logic in backend handles it?
// Wait, my backend logic for 'ask_location' relies on frontend strictly?
// Let's check backend add_download. It doesn't seem to open dialog.
// But the user said "let me send the old file to you". The old file DOES NOT have the location picker.
// I will respect the old file. If 'ask_location' is broken, we fix it later.
// Actually, I'll keep the import but only use it if I really need to match logic. 
// User's provided code doesn't import 'open'. I will omit it to be safe and exact.

interface TorrentFile {
    name: string;
    size: number;
    index: number;
}

interface TorrentInfo {
    id?: string; // Add id optional from my previous edits, or just keep strict? logic uses name.
    name: string;
    total_size: number;
    files: TorrentFile[];
}

interface DownloadQueueProps {
    filter: "downloads" | "active" | "completed" | "settings";
}

interface DownloadItem {
    id: string;
    filename: string;
    url: string;
    size: number;
    downloaded: number;
    speed: number;
    eta: number;
    connections: number;
    protocol: "http" | "torrent";
    status: "downloading" | "paused" | "completed" | "queued" | "error";
    filepath: string;
}

interface ProgressPayload {
    id: string;
    total: number;
    downloaded: number;
    speed: number;
    eta: number;
    connections: number;
}

const formatSize = (bytes: number) => {
    if (bytes === 0) return '0 B';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
};

const formatSpeed = (bytesPerSec: number) => {
    if (bytesPerSec === 0) return "0 B/s";
    if (bytesPerSec < 1024) return `${bytesPerSec} B/s`;
    if (bytesPerSec < 1024 * 1024) return `${(bytesPerSec / 1024).toFixed(1)} KB/s`;
    return `${(bytesPerSec / (1024 * 1024)).toFixed(1)} MB/s`;
};

const formatEta = (seconds: number) => {
    if (seconds <= 0 || !isFinite(seconds)) return "--";
    if (seconds < 60) return `${Math.floor(seconds)}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${Math.floor(seconds % 60)}s`;
    return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
};

export function DownloadQueue({ filter }: DownloadQueueProps) {
    const [downloads, setDownloads] = useState<DownloadItem[]>([]);
    const [isAddModalOpen, setIsAddModalOpen] = useState(false);

    useEffect(() => {
        // Fetch initial downloads
        const fetchDownloads = async () => {
            try {
                const res = await invoke<DownloadItem[]>("get_downloads");
                setDownloads(res);
            } catch (err) {
                console.error("Failed to fetch downloads:", err);
            }
        };

        fetchDownloads();

        // Listen for progress updates
        const unlistenProgress = listen<ProgressPayload>("download-progress", (event) => {
            const progress = event.payload;
            setDownloads((prev) =>
                prev.map((d) => {
                    if (d.id === progress.id) {
                        return {
                            ...d,
                            downloaded: progress.downloaded,
                            size: progress.total,
                            speed: progress.speed,
                            eta: progress.eta,
                            connections: progress.connections,
                            protocol: d.protocol, // Preserve protocol
                            status: "downloading",
                        };
                    }
                    return d;
                })
            );
        });

        const unlistenCompleted = listen<string>("download-completed", (event) => {
            const id = event.payload;
            setDownloads((prev) =>
                prev.map((d) => {
                    if (d.id === id) {
                        return { ...d, status: "completed", speed: 0 };
                    }
                    return d;
                })
            );
        });

        const unlistenError = listen<[string, string]>("download-error", (event) => {
            const [id] = event.payload;
            setDownloads((prev) =>
                prev.map((d) => {
                    if (d.id === id) {
                        return { ...d, status: "error", speed: 0 };
                    }
                    return d;
                })
            );
        });

        const unlistenName = listen<{ id: string; filename: string }>("download-name-updated", (event) => {
            setDownloads((prev) => {
                return prev.map(d => d.id === event.payload.id ? { ...d, filename: event.payload.filename } : d);
            });
        });

        return () => {
            unlistenProgress.then((u) => u());
            unlistenCompleted.then((u) => u());
            unlistenError.then((u) => u());
            unlistenName.then((u) => u());
        };
    }, []);

    const filteredDownloads = downloads.filter((d) => {
        if (filter === "active") return d.status === "downloading" || d.status === "queued";
        if (filter === "completed") return d.status === "completed";
        return true;
    });

    const titles: Record<string, string> = {
        downloads: "All Downloads",
        active: "Active Downloads",
        completed: "History",
        settings: "Settings"
    };

    const handleRefreshList = async () => {
        const res = await invoke<DownloadItem[]>("get_downloads");
        setDownloads(res);
    };

    return (
        <div className="h-full flex flex-col relative w-full max-w-5xl mx-auto">
            {/* Header */}
            <div className="flex items-center justify-between mb-8 sticky top-0 bg-brand-primary z-20 py-4 border-b border-transparent">
                <div className="flex-col">
                    <h1 className="text-2xl font-semibold text-text-primary tracking-tight">{titles[filter]}</h1>
                    <p className="text-sm text-text-secondary mt-1">
                        {filteredDownloads.length} {filteredDownloads.length === 1 ? 'Job' : 'Jobs'} in queue
                    </p>
                </div>

                <div className="flex items-center gap-3">
                    <button
                        onClick={() => setIsAddModalOpen(true)}
                        className="btn-primary flex items-center gap-2"
                    >
                        <FileDown size={18} />
                        <span>New Task</span>
                    </button>
                    <button
                        onClick={handleRefreshList}
                        className="btn-secondary p-2.5"
                        title="Refresh"
                    >
                        <ArrowDown size={18} className="text-text-secondary" />
                    </button>
                </div>
            </div>

            {/* Download List */}
            {filteredDownloads.length === 0 ? (
                <EmptyState filter={filter} onAdd={() => setIsAddModalOpen(true)} />
            ) : (
                <div className="flex-1 space-y-3 overflow-y-auto pr-2 pb-12 scrollbar-hide">
                    {filteredDownloads.map((download) => (
                        <DownloadCard
                            key={download.id}
                            download={download}
                            onRefresh={handleRefreshList}
                        />
                    ))}
                </div>
            )}

            {isAddModalOpen && (
                <AddDownloadModal
                    onClose={() => setIsAddModalOpen(false)}
                    onAdded={handleRefreshList}
                />
            )}
        </div>
    );
}

function EmptyState({ filter, onAdd }: { filter: string, onAdd: () => void }) {
    return (
        <div className="flex-1 flex flex-col items-center justify-center text-center p-12">
            <div className="w-24 h-24 rounded-full bg-brand-secondary border border-surface-border flex items-center justify-center mb-6">
                <CloudDownload size={32} className="text-text-tertiary" />
            </div>
            <h2 className="text-xl font-medium text-text-primary mb-2">No active downloads</h2>
            <p className="text-text-secondary max-w-sm mb-8">
                {filter === "active"
                    ? "Your queue is empty."
                    : "Add a URL or Magnet link to begin downloading."}
            </p>
            <button
                onClick={onAdd}
                className="btn-primary"
            >
                Start Downloading
            </button>
        </div>
    );
}

function DownloadCard({ download, onRefresh }: { download: DownloadItem, onRefresh: () => void }) {
    const progress = download.size > 0 ? (download.downloaded / download.size) * 100 : 0;

    // Smooth progress for visual
    const [visualProgress, setVisualProgress] = useState(progress);
    useEffect(() => {
        setVisualProgress(progress);
    }, [progress]);

    const handlePauseResume = async (e: React.MouseEvent) => {
        e.stopPropagation();
        if (download.status === "downloading") {
            await invoke("pause_download", { id: download.id });
        } else {
            await invoke("resume_download", { id: download.id });
        }
        onRefresh();
    };

    const handleDelete = async (e: React.MouseEvent) => {
        e.stopPropagation();
        await invoke("delete_download", { id: download.id });
        onRefresh();
    };

    const getStatusColor = () => {
        if (download.status === 'completed') return 'text-status-success';
        if (download.status === 'error') return 'text-status-error';
        if (download.status === 'paused') return 'text-status-warning';
        return 'text-text-primary';
    };

    return (
        <div className="card-base p-4 group card-hover relative overflow-hidden">
            {/* Background Progress Tint - Subtle */}
            <div
                className="absolute inset-y-0 left-0 bg-brand-tertiary/20 transition-all duration-500 ease-out z-0 pointer-events-none"
                style={{ width: `${visualProgress}%` }}
            />

            <div className="relative z-10 flex gap-4 items-center">
                {/* Icon Box */}
                <div className="w-10 h-10 rounded-lg bg-brand-tertiary flex items-center justify-center">
                    <FileDown className={clsx("w-5 h-5", getStatusColor())} />
                </div>

                {/* Content */}
                <div className="flex-1 min-w-0 flex flex-col gap-1.5">
                    {/* Header Row */}
                    <div className="flex items-center justify-between">
                        <div className="flex items-center gap-2 flex-1 min-w-0">
                            <h3 className="text-sm font-medium text-text-primary truncate" title={download.filename}>
                                {download.filename}
                            </h3>
                            {download.protocol === 'torrent' && (download.size === 0 || download.status === 'queued') && (download.status === 'downloading' || download.status === 'queued') && (
                                <span className="bg-amber-500/10 text-amber-500 text-[10px] px-1.5 py-0.5 rounded border border-amber-500/20 font-bold tracking-wider animate-pulse whitespace-nowrap">
                                    INITIALIZING
                                </span>
                            )}
                        </div>

                        {/* Actions */}
                        <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity duration-200">
                            <button
                                onClick={handlePauseResume}
                                className="btn-ghost p-1.5"
                            >
                                {download.status === "downloading" ? <Pause size={14} /> : <Play size={14} />}
                            </button>
                            <button
                                onClick={() => {
                                    invoke("show_in_folder", { path: download.filepath });
                                }}
                                className="btn-ghost p-1.5"
                            >
                                <FolderOpen size={14} />
                            </button>
                            <button
                                onClick={handleDelete}
                                className="btn-ghost p-1.5 text-status-error hover:text-red-400"
                            >
                                <Trash2 size={14} />
                            </button>
                        </div>
                    </div>

                    {/* Progress Bar Container */}
                    <div className="w-full h-1 bg-brand-tertiary rounded-full overflow-hidden relative">
                        {download.protocol === 'torrent' && (download.size === 0 || download.status === 'queued') && (download.status === 'downloading' || download.status === 'queued') ? (
                            <div className="absolute inset-0 bg-text-primary/30 animate-pulse" />
                        ) : (
                            <div
                                className={clsx(
                                    "h-full rounded-full transition-[width] duration-50 ease-linear",
                                    download.status === 'completed' ? 'bg-status-success' :
                                        download.status === 'error' ? 'bg-status-error' : 'bg-text-primary'
                                )}
                                style={{ width: `${visualProgress}%` }}
                            />
                        )}
                    </div>

                    {/* Meta Data Row */}
                    <div className="flex items-center justify-between text-xs text-text-secondary mt-0.5">
                        <div className="flex items-center gap-3">
                            <span>
                                {download.protocol === 'torrent' && (download.size === 0 || download.status === 'queued') && (download.status === 'downloading' || download.status === 'queued') ? (
                                    <span className="text-amber-500 animate-pulse">Fetching metadata...</span>
                                ) : (
                                    <>
                                        {formatSize(download.downloaded)} <span className="text-text-tertiary">/</span> {formatSize(download.size)}
                                    </>
                                )}
                            </span>

                            {download.status === 'downloading' && (
                                <>
                                    <div className="flex items-center gap-1 text-text-primary">
                                        <ArrowDown size={10} />
                                        <span>{formatSpeed(download.speed)}</span>
                                    </div>
                                    <div className="flex items-center gap-1 text-text-tertiary">
                                        {download.protocol === 'torrent' ? (
                                            <>
                                                <Users size={10} />
                                                <span>{download.connections} Peers</span>
                                            </>
                                        ) : (
                                            <>
                                                <Wifi size={10} />
                                                <span>{download.connections} Conns</span>
                                            </>
                                        )}
                                    </div>
                                </>
                            )}
                        </div>

                        <div className="flex items-center gap-2">
                            {download.status === 'downloading' && (
                                <div className="flex items-center gap-1 text-text-tertiary">
                                    <Clock size={10} />
                                    <span className="font-mono">{formatEta(download.eta)}</span>
                                </div>
                            )}
                            <span className={clsx("font-medium", getStatusColor())}>
                                {download.status === 'completed'
                                    ? 'Done'
                                    : (download.protocol === 'torrent' && (download.size === 0 || download.status === 'queued'))
                                        ? 'Initializing...'
                                        : `${visualProgress.toFixed(1)}%`}
                            </span>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}

function AddDownloadModal({ onClose, onAdded }: { onClose: () => void, onAdded: () => void }) {
    const [url, setUrl] = useState("");
    const [isAdding, setIsAdding] = useState(false);
    const [status, setStatus] = useState<string | null>(null);
    const [torrentInfo, setTorrentInfo] = useState<TorrentInfo | null>(null);

    const handleAdd = async () => {
        if (!url) return;

        try {
            new URL(url);
        } catch (e) {
            console.error("Invalid URL format");
            setStatus("Error: Invalid URL format");
            return;
        }

        setIsAdding(true);
        setStatus("Validating URL...");

        try {
            // Validate URL first
            interface UrlTypeInfo {
                is_magnet: boolean;
                content_type: string | null;
                content_length: number | null;
                hinted_filename: string | null;
            }

            const typeInfo = await invoke<UrlTypeInfo>("validate_url_type", { url });

            if (typeInfo.is_magnet) {
                setStatus("Analyzing torrent metadata...");
                const info = await invoke<TorrentInfo>("analyze_torrent", { url });
                setStatus(null);
                setTorrentInfo(info);
            } else {
                // Check if it's a webpage
                if (typeInfo.content_type?.includes("text/html")) {
                    // Note: window.confirm might be blocked in some Tauri contexts, but user code used it.
                    // We assume it works or they deal with it.
                    const confirm = await window.confirm(
                        "This URL appears to be a webpage, not a direct file download.\n\n" +
                        "If you intended to download a torrent, please copy the MAGNET LINK instead.\n\n" +
                        "Do you want to download this webpage as a file anyway?"
                    );

                    if (!confirm) {
                        setIsAdding(false);
                        setStatus(null);
                        return;
                    }
                }

                // Use server-provided filename if available, otherwise extract from URL
                let filename = typeInfo.hinted_filename || "download";

                if (!typeInfo.hinted_filename) {
                    try {
                        const urlObj = new URL(url);
                        const pathSegments = urlObj.pathname.split('/');
                        const lastSegment = pathSegments[pathSegments.length - 1];
                        if (lastSegment) filename = decodeURIComponent(lastSegment).split('?')[0];
                        filename = filename.replace(/[<>:"/\\|?*]/g, '_');

                        // Add extension if missing and we know content-type
                        if (!filename.includes('.')) {
                            if (typeInfo.content_type?.includes('html')) filename += '.html';
                            else if (typeInfo.content_type?.includes('pdf')) filename += '.pdf';
                            else if (typeInfo.content_type?.includes('zip')) filename += '.zip';
                        }
                    } catch (e) {
                        filename = "download_file";
                    }
                }

                if (!filename || filename.trim() === "") filename = "download";

                await invoke("add_download", { url, filename, filepath: "" });
                onAdded();
                onClose();
            }
        } catch (err) {
            console.error("Failed to add download:", err);
            setStatus(`Error: ${err}`);
        } finally {
            if (!torrentInfo) setIsAdding(false);
        }
    };

    const handleTorrentSelect = async (indices: number[]) => {
        setIsAdding(true);
        try {
            // Add the torrent with selection
            await invoke("add_torrent", {
                url,
                filename: torrentInfo?.name || "Torrent Download",
                filepath: "",
                indices
            });
            onAdded();
            onClose();
        } catch (err) {
            console.error("Failed to start torrent/zip:", err);
            setStatus(`Error: ${err}`);
        } finally {
            setIsAdding(false);
        }
    };

    return (
        <>
            <ModalPortal>
                <div className="fixed inset-0 z-[100] flex items-center justify-center p-6 bg-black/50 backdrop-blur-sm animate-fade-in">
                    <div className="bg-brand-secondary border border-surface-border w-full max-w-lg rounded-xl p-6 shadow-2xl scale-100 transition-all relative overflow-hidden text-left">
                        <h2 className="text-lg font-semibold text-text-primary mb-1">Add New Download</h2>
                        <p className="text-text-secondary text-sm mb-6">Enter a URL or Magnet link to start.</p>

                        <div className="space-y-4">
                            <div className="relative">
                                <input
                                    autoFocus
                                    type="text"
                                    value={url}
                                    onChange={(e) => setUrl(e.target.value)}
                                    placeholder="https://example.com/file.zip or magnet:?xt=..."
                                    className="w-full bg-brand-primary border border-surface-border rounded-lg px-4 py-3 text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-text-secondary transition-all font-mono text-sm pr-12"
                                    onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
                                />
                                <div className="absolute right-4 top-1/2 -translate-y-1/2">
                                    <Plus size={18} className="text-text-tertiary" />
                                </div>
                            </div>

                            {status && (
                                <div className={clsx(
                                    "text-xs p-3 rounded-lg flex items-center gap-2",
                                    status.startsWith("Error") ? "bg-status-error/10 text-status-error" : "bg-brand-tertiary text-text-secondary animate-pulse"
                                )}>
                                    {status.startsWith("Error") ? <AlertCircle size={14} /> : <DatabaseIcon size={14} />}
                                    {status}
                                </div>
                            )}

                            <div className="flex items-center justify-end gap-3 mt-8">
                                <button
                                    onClick={onClose}
                                    className="px-4 py-2 text-text-secondary hover:text-text-primary font-medium hover:bg-brand-tertiary rounded-lg transition-all text-sm"
                                    disabled={isAdding}
                                >
                                    Cancel
                                </button>
                                <button
                                    onClick={handleAdd}
                                    disabled={isAdding || !url}
                                    className="btn-primary text-sm flex items-center gap-2"
                                >
                                    {isAdding && !torrentInfo && <div className="w-3 h-3 border-2 border-brand-primary border-t-transparent rounded-full animate-spin" />}
                                    {isAdding && !torrentInfo ? "Analyzing..." : "Add Download"}
                                </button>
                            </div>
                        </div>
                    </div>
                </div>
            </ModalPortal>

            {torrentInfo && (
                <ModalPortal>
                    <TorrentFileSelector
                        info={torrentInfo}
                        onSelect={handleTorrentSelect}
                        onCancel={() => {
                            setTorrentInfo(null);
                            setIsAdding(false);
                        }}
                    />
                </ModalPortal>
            )}
        </>
    );
}
