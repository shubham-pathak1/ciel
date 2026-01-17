import React, { useState, useEffect, useCallback, memo } from "react";
import { CloudDownload, FileDown, Pause, Trash2, FolderOpen, Play, ArrowDown, Clock, Users, Wifi, Video as VideoIcon, Database as DatabaseIcon } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { motion, AnimatePresence } from "framer-motion";
import clsx from "clsx";
import { ModalPortal } from "./ModalPortal";
import { TorrentFileSelector } from "./TorrentFileSelector";
import { VideoPreview } from "./VideoPreview";

interface TorrentFile {
    name: string;
    size: number;
    index: number;
}

interface TorrentInfo {
    id?: string;
    name: string;
    total_size: number;
    files: TorrentFile[];
}

interface DownloadQueueProps {
    filter: "downloads" | "active" | "completed";
    category?: string;
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
    protocol: "http" | "torrent" | "video";
    status: "downloading" | "paused" | "completed" | "queued" | "error";
    filepath: string;
    status_text?: string;
    metadata: string | null;
    user_agent: string | null;
    cookies: string | null;
    category: string;
}

interface ProgressPayload {
    id: string;
    total: number;
    downloaded: number;
    speed: number;
    eta: number;
    connections: number;
    status_text?: string;
}

const formatSize = (bytes: number) => {
    if (bytes === 0) return '0 B';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${((bytes / 1024)).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${((bytes / (1024 * 1024))).toFixed(1)} MB`;
    return `${((bytes / (1024 * 1024 * 1024))).toFixed(2)} GB`;
};

const formatSpeed = (bytesPerSec: number) => {
    if (bytesPerSec === 0) return "0 B/s";
    if (bytesPerSec < 1024) return `${bytesPerSec} B/s`;
    if (bytesPerSec < 1024 * 1024) return `${((bytesPerSec / 1024)).toFixed(1)} KB/s`;
    return `${((bytesPerSec / (1024 * 1024))).toFixed(1)} MB/s`;
};

const formatEta = (seconds: number) => {
    if (seconds <= 0 || !isFinite(seconds)) return "--";
    if (seconds < 60) return `${Math.floor(seconds)}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${Math.floor(seconds % 60)}s`;
    return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
};

export function DownloadQueue({ filter, category }: DownloadQueueProps) {
    const [downloads, setDownloads] = useState<DownloadItem[]>([]);
    const [isAddModalOpen, setIsAddModalOpen] = useState(false);
    const [autocatchUrl, setAutocatchUrl] = useState("");

    const handleRefreshList = useCallback(async () => {
        try {
            const [downloads, settings] = await Promise.all([
                invoke<DownloadItem[]>("get_downloads"),
                invoke<{ auto_resume?: string }>("get_settings")
            ]);
            setDownloads(downloads);

            // Auto-resume logic: if app was closed while downloading, resume them
            if (settings.auto_resume === "true") {
                downloads.forEach((d) => {
                    if (d.status === "downloading") {
                        invoke("resume_download", { id: d.id }).catch(console.error);
                    }
                });
            }
        } catch (err) {
            console.error("Failed to fetch downloads:", err);
        }
    }, []);

    const playSuccessSound = () => {
        try {
            const context = new (window.AudioContext || (window as any).webkitAudioContext)();
            const oscillator = context.createOscillator();
            const gain = context.createGain();

            oscillator.type = 'sine';
            oscillator.frequency.setValueAtTime(587.33, context.currentTime); // D5
            oscillator.frequency.exponentialRampToValueAtTime(880.00, context.currentTime + 0.1); // A5

            gain.gain.setValueAtTime(0, context.currentTime);
            gain.gain.linearRampToValueAtTime(0.2, context.currentTime + 0.05);
            gain.gain.exponentialRampToValueAtTime(0.01, context.currentTime + 0.4);

            oscillator.connect(gain);
            gain.connect(context.destination);

            oscillator.start(context.currentTime);
            oscillator.stop(context.currentTime + 0.4);
        } catch (e) {
            console.error("Failed to play sound:", e);
        }
    };

    useEffect(() => {
        handleRefreshList();

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
                            status: "downloading",
                            status_text: progress.status_text,
                        };
                    }
                    return d;
                })
            );
        });

        const unlistenCompleted = listen<string>("download-completed", async () => {
            handleRefreshList();
            try {
                const settings = await invoke<Record<string, string>>("get_settings");
                if (settings.sound_on_finish === "true") {
                    playSuccessSound();
                }
            } catch (err) {
                console.error("Failed to check sound setting:", err);
            }
        });

        const unlistenName = listen<{ id: string; filename: string }>("download-name-updated", (event) => {
            setDownloads((prev) => prev.map(d => d.id === event.payload.id ? { ...d, filename: event.payload.filename } : d));
        });

        const unlistenAutocatch = listen<string>("autocatch-url", async (event) => {
            try {
                const settings = await invoke<Record<string, string>>("get_settings");
                if (settings.autocatch_enabled === "true") {
                    setAutocatchUrl(event.payload);
                }
            } catch (err) {
                console.error("Failed to check autocatch setting:", err);
            }
        });

        return () => {
            unlistenProgress.then((u) => u());
            unlistenCompleted.then((u) => u());
            unlistenName.then((u) => u());
            unlistenAutocatch.then((u) => u());
        };
    }, []);


    const filteredDownloads = downloads.filter((d) => {
        if (filter === "active") return d.status === "downloading" || d.status === "queued";
        if (filter === "completed") return d.status === "completed";
        if (category && category !== "All") return d.category === category;
        return true;
    });

    return (
        <div className="flex flex-col h-full bg-brand-primary/50 relative overflow-hidden">
            <div className="p-8 pb-4 flex items-center justify-between relative z-10">
                <div className="flex flex-col gap-1">
                    <h1 className="text-2xl font-bold text-text-primary tracking-tight flex items-center gap-2">
                        {category ? `${category} Downloads` : filter === "active" ? "Active Downloads" : "All Downloads"}
                        {filteredDownloads.length > 0 && (
                            <span className="text-xs font-medium px-2 py-0.5 rounded-full bg-brand-tertiary/20 text-brand-secondary">
                                {filteredDownloads.length}
                            </span>
                        )}
                    </h1>
                    <p className="text-sm text-text-tertiary">
                        {category ? `Organized collection of ${category.toLowerCase()} files` : "Manage and track your download tasks in real-time"}
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
                    <AnimatePresence mode="popLayout" initial={false}>
                        {filteredDownloads.map((download) => (
                            <DownloadCard
                                key={download.id}
                                download={download}
                                onRefresh={handleRefreshList}
                            />
                        ))}
                    </AnimatePresence>
                </div>
            )}

            <AnimatePresence>
                {isAddModalOpen && (
                    <AddDownloadModal
                        onClose={() => setIsAddModalOpen(false)}
                        onAdded={handleRefreshList}
                        initialUrl={autocatchUrl}
                    />
                )}
            </AnimatePresence>

            {/* Removed AutocatchNotification */}
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

const DownloadCard = memo(React.forwardRef<HTMLDivElement, { download: DownloadItem, onRefresh: () => void }>(
    ({ download, onRefresh }, ref) => {
        const progress = download.size > 0 ? (download.downloaded / download.size) * 100 : 0;
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

        const ProtocolIcon = () => {
            if (download.protocol === 'video') return <VideoIcon className={clsx("w-5 h-5", getStatusColor())} />;
            return <FileDown className={clsx("w-5 h-5", getStatusColor())} />;
        };

        return (
            <motion.div
                ref={ref}
                layout
                initial={{ opacity: 0, y: 5, scale: 0.99 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, scale: 0.98, transition: { duration: 0.1 } }}
                className="card-base p-4 group card-hover relative overflow-hidden"
            >
                <motion.div
                    layout
                    className="absolute inset-y-0 left-0 bg-brand-tertiary/20 z-0 pointer-events-none"
                    style={{ width: `${visualProgress}%` }}
                    transition={{ type: "spring", stiffness: 400, damping: 40 }}
                />

                <div className="relative z-10 flex gap-4 items-center">
                    <div className="w-10 h-10 rounded-lg bg-brand-tertiary flex items-center justify-center">
                        <ProtocolIcon />
                    </div>

                    <div className="flex-1 min-w-0 flex flex-col gap-1.5">
                        <div className="flex items-center justify-between">
                            <h3 className="text-sm font-medium text-text-primary truncate" title={download.filename}>
                                {download.filename}
                            </h3>
                            <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                                <button onClick={handlePauseResume} className="btn-ghost p-1.5">
                                    {download.status === "downloading" ? <Pause size={14} /> : <Play size={14} />}
                                </button>
                                <button onClick={() => invoke("show_in_folder", { path: download.filepath })} className="btn-ghost p-1.5">
                                    <FolderOpen size={14} />
                                </button>
                                <button onClick={handleDelete} className="btn-ghost p-1.5 text-status-error">
                                    <Trash2 size={14} />
                                </button>
                            </div>
                        </div>

                        <div className="w-full h-1 bg-brand-tertiary rounded-full overflow-hidden relative">
                            <motion.div
                                layout
                                className={clsx(
                                    "h-full rounded-full transition-all duration-500",
                                    download.status === 'completed' ? 'bg-status-success' :
                                        download.status === 'error' ? 'bg-status-error' :
                                            download.status_text && download.status_text !== 'Starting...'
                                                ? 'bg-brand-primary animate-progress-indeterminate bg-[length:1rem_1rem] bg-gradient-to-r from-brand-primary via-brand-secondary to-brand-primary'
                                                : 'bg-text-primary'
                                )}
                                style={{ width: `${download.status_text && download.status_text !== 'Starting...' ? 100 : visualProgress}%` }}
                                transition={{ type: "spring", stiffness: 400, damping: 40 }}
                            />
                        </div>

                        <div className="flex items-center justify-between text-xs text-text-secondary mt-0.5">
                            <div className="flex items-center gap-3">
                                {download.status_text && (
                                    <span className="text-white font-medium animate-pulse">{download.status_text}</span>
                                )}
                                {!download.status_text && (
                                    <span className="font-medium tracking-wide">
                                        {formatSize(download.downloaded)} <span className="text-text-tertiary font-normal px-1">of</span> {formatSize(download.size)}
                                    </span>
                                )}

                                {download.status === 'downloading' && !download.status_text && (
                                    <>
                                        <div className="flex items-center gap-1 text-text-primary">
                                            <ArrowDown size={10} />
                                            <span>{formatSpeed(download.speed)}</span>
                                        </div>
                                        <div className="flex items-center gap-1 text-text-tertiary">
                                            {download.protocol === 'torrent' ? <Users size={10} /> : <Wifi size={10} />}
                                            <span>{download.connections}</span>
                                        </div>
                                    </>
                                )}
                            </div>
                            <div className="flex items-center gap-2">
                                {download.status === 'downloading' && (
                                    <div className="flex items-center gap-1 text-text-tertiary">
                                        <Clock size={10} />
                                        <span className="font-mono text-[10px] tracking-tight">{formatEta(download.eta)} remaining</span>
                                    </div>
                                )}
                                <span className={clsx("font-medium", getStatusColor())}>
                                    {download.status === 'completed' ? 'Done' : `${visualProgress.toFixed(1)}%`}
                                </span>
                            </div>
                        </div>
                    </div>
                </div>
            </motion.div>
        );
    }
));

function AddDownloadModal({ onClose, onAdded, initialUrl = "" }: { onClose: () => void, onAdded: () => void, initialUrl?: string }) {
    const [url, setUrl] = useState(initialUrl);
    const [isAdding, setIsAdding] = useState(false);
    const [status, setStatus] = useState<string | null>(null);
    const [torrentInfo, setTorrentInfo] = useState<TorrentInfo | null>(null);
    const [videoMetadata, setVideoMetadata] = useState<any | null>(null);
    const [showAdvanced, setShowAdvanced] = useState(false);
    const [userAgent, setUserAgent] = useState("");
    const [cookies, setCookies] = useState("");

    useEffect(() => {
        const checkClipboard = async () => {
            // Priority 1: Explicitly passed initialUrl from parent (Autocatch event)
            if (initialUrl && initialUrl !== url) {
                setUrl(initialUrl);
                return;
            }

            // Priority 2: Direct clipboard check on mount if field is empty
            if (!url) {
                try {
                    const settings = await invoke<Record<string, string>>("get_settings");
                    if (settings.autocatch_enabled === "true") {
                        const clipText = await invoke<string | null>("get_clipboard");
                        if (clipText) {
                            setUrl(clipText);
                        }
                    }
                } catch (e) {
                    // Silent failure
                }
            }
        };
        checkClipboard();
    }, [initialUrl]);

    const handleAdd = async () => {
        if (!url) return;
        try { new URL(url); } catch (e) { setStatus("Error: Invalid URL"); return; }

        setIsAdding(true);
        setStatus("Analyzing...");

        try {
            const isVideoSite = /youtube\.com|youtu\.be|twitter\.com|x\.com|instagram\.com|vimeo\.com|facebook\.com/.test(url);
            if (isVideoSite) {
                try {
                    const metadata = await invoke("analyze_video_url", { url });
                    setVideoMetadata(metadata);
                    setStatus(null);
                    return;
                } catch (e) { console.warn("Video analysis failed", e); }
            }

            const typeInfo = await invoke<any>("validate_url_type", { url });
            if (typeInfo.is_magnet) {
                const info = await invoke<TorrentInfo>("analyze_torrent", { url });
                setTorrentInfo(info);
                setStatus(null);
            } else {
                await invoke("add_download", {
                    url,
                    filename: typeInfo.hinted_filename || "download",
                    filepath: "",
                    userAgent: userAgent || null,
                    cookies: cookies || null
                });
                onAdded();
                onClose();
            }
        } catch (err) {
            setStatus(`Error: ${err}`);
        } finally {
            if (!torrentInfo && !videoMetadata) setIsAdding(false);
        }
    };

    const handleVideoDownload = async (formatId: string, ext: string, audioId?: string, totalSize?: number) => {
        setIsAdding(true);
        try {
            const filename = `${videoMetadata.title.replace(/[<>:"/\\|?*]/g, '_')}.${ext}`;
            await invoke("add_video_download", { url, formatId, audioId, totalSize, filepath: filename });
            onAdded();
            onClose();
        } catch (err) {
            setStatus(`Error: ${err}`);
        } finally {
            setIsAdding(false);
        }
    };

    const handleTorrentSelect = async (indices: number[]) => {
        setIsAdding(true);
        try {
            await invoke("add_torrent", { url, filename: torrentInfo?.name || "Torrent", filepath: "", indices });
            onAdded();
            onClose();
        } catch (err) {
            setStatus(`Error: ${err}`);
        } finally {
            setIsAdding(false);
        }
    };

    return (
        <>
            <ModalPortal>
                <div className="fixed inset-0 z-[100] flex items-center justify-center p-6 bg-black/50 backdrop-blur-sm">
                    <motion.div
                        initial={{ opacity: 0, scale: 0.98 }}
                        animate={{ opacity: 1, scale: 1 }}
                        exit={{ opacity: 0, scale: 0.98 }}
                        className="bg-brand-secondary border border-surface-border w-full max-w-lg rounded-xl p-6 shadow-2xl relative text-left"
                    >
                        <h2 className="text-lg font-semibold text-text-primary mb-1">
                            {videoMetadata ? "Video Options" : "Add New Download"}
                        </h2>
                        <p className="text-text-secondary text-sm mb-6">
                            {videoMetadata ? "Select quality." : "Enter a URL or Magnet link."}
                        </p>

                        {videoMetadata ? (
                            <VideoPreview
                                metadata={videoMetadata}
                                onCancel={() => { setVideoMetadata(null); setIsAdding(false); }}
                                onDownload={handleVideoDownload}
                            />
                        ) : (
                            <div className="space-y-4">
                                <input
                                    autoFocus
                                    type="text"
                                    value={url}
                                    onChange={(e) => setUrl(e.target.value)}
                                    placeholder="https://..."
                                    className="w-full bg-brand-primary border border-surface-border rounded-lg px-4 py-3 text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono text-sm"
                                    onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
                                />

                                <div className="pt-2">
                                    <button
                                        onClick={() => setShowAdvanced(!showAdvanced)}
                                        className="text-[10px] items-center gap-1.5 uppercase font-bold tracking-wider text-text-tertiary hover:text-text-secondary transition-colors inline-flex mb-3"
                                    >
                                        <div className={clsx("w-1 h-3 rounded-full bg-brand-tertiary", showAdvanced && "bg-text-secondary")} />
                                        Advanced Settings
                                    </button>

                                    <AnimatePresence>
                                        {showAdvanced && (
                                            <motion.div
                                                initial={{ height: 0, opacity: 0 }}
                                                animate={{ height: "auto", opacity: 1 }}
                                                exit={{ height: 0, opacity: 0 }}
                                                className="overflow-hidden space-y-4"
                                            >
                                                <div className="space-y-1">
                                                    <label className="text-[10px] text-text-tertiary ml-1 uppercase tracking-widest font-bold">User-Agent</label>
                                                    <input
                                                        type="text"
                                                        value={userAgent}
                                                        onChange={(e) => setUserAgent(e.target.value)}
                                                        placeholder="Mozilla/5.0..."
                                                        className="w-full bg-brand-primary border border-surface-border rounded-lg px-3 py-2 text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono text-xs"
                                                    />
                                                </div>
                                                <div className="space-y-1">
                                                    <label className="text-[10px] text-text-tertiary ml-1 uppercase tracking-widest font-bold">Cookies (Raw String)</label>
                                                    <textarea
                                                        value={cookies}
                                                        onChange={(e) => setCookies(e.target.value)}
                                                        placeholder="session=...; _uid=..."
                                                        className="w-full h-20 bg-brand-primary border border-surface-border rounded-lg px-3 py-2 text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono text-xs resize-none"
                                                    />
                                                </div>
                                            </motion.div>
                                        )}
                                    </AnimatePresence>
                                </div>
                                {status && (
                                    <div className="text-xs p-3 rounded-lg bg-brand-tertiary text-text-secondary flex items-center gap-2">
                                        <DatabaseIcon size={14} />
                                        {status}
                                    </div>
                                )}
                                <div className="flex justify-end gap-3 mt-8">
                                    <button onClick={onClose} className="px-4 py-2 text-text-secondary text-sm" disabled={isAdding}>Cancel</button>
                                    <button onClick={handleAdd} disabled={isAdding || !url} className="btn-primary text-sm">
                                        {isAdding ? "Analyzing..." : "Add Download"}
                                    </button>
                                </div>
                            </div>
                        )}
                    </motion.div>
                </div>
            </ModalPortal>

            {torrentInfo && (
                <ModalPortal>
                    <TorrentFileSelector
                        info={torrentInfo}
                        onSelect={handleTorrentSelect}
                        onCancel={() => { setTorrentInfo(null); setIsAdding(false); }}
                    />
                </ModalPortal>
            )}
        </>
    );
}

