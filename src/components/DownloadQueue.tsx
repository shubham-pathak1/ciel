/**
 * @file DownloadQueue.tsx
 * @description The primary view for managing the download queue. 
 * Orchestrates HTTP, Torrent, and Video downloads by communicating with the Tauri backend.
 */

import React, { useState, useEffect, useCallback, useRef, memo } from "react";
import { CloudDownload, FileDown, Pause, Trash2, FolderOpen, Play, ArrowDown, Clock, Users, Wifi, Database as DatabaseIcon, ChevronDown } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { motion, AnimatePresence } from "framer-motion";
import clsx from "clsx";
import { open } from "@tauri-apps/plugin-dialog";
import { useSettings } from "../hooks/useSettings";
import { ModalPortal } from "./ModalPortal";
import { ConfirmDialog } from "./ConfirmDialog";
import { TorrentFileSelector } from "./TorrentFileSelector";

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

/**
 * Props for the DownloadQueue component.
 * @property filter - Restricts the visible downloads by their current status.
 * @property category - Optional tag to further narrow down the displayed list.
 */
interface DownloadQueueProps {
    filter: "downloads" | "active" | "completed";
    category?: string;
}

/**
 * Represents a single download record from the database.
 */
interface DownloadItem {
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

interface ProgressPayload {
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

const formatPhaseElapsed = (seconds?: number) => {
    if (seconds === undefined || seconds < 0 || !isFinite(seconds)) return "";
    if (seconds < 60) return `${Math.floor(seconds)}s`;
    return `${Math.floor(seconds / 60)}m ${Math.floor(seconds % 60)}s`;
};

const getPhaseHint = (phase?: string) => {
    switch (phase) {
        case "restoring_session":
            return "restoring";
        case "resuming":
            return "resuming";
        case "verifying_data":
            return "checking files";
        case "finding_peers":
            return "searching peers";
        case "connecting":
            return "connecting";
        case "negotiating_peers":
            return "negotiating peers";
        case "preparing_first_piece":
            return "receiving data";
        case "fetching_metadata":
            return "loading metadata";
        default:
            return "";
    }
};

/**
 * Main Download Queue Component.
 * 
 * Responsibilities:
 * - Fetches initial download list from the backend.
 * - Listens for real-time progress events over the Tauri IPC bridge.
 * - Handles user interactions for pausing, resuming, and deleting downloads.
 * - Manages "Auto-Catch" modal triggers when interesting URLs are found in the clipboard.
 */
export function DownloadQueue({ filter, category }: DownloadQueueProps) {
    const [downloads, setDownloads] = useState<DownloadItem[]>([]);
    const [isAddModalOpen, setIsAddModalOpen] = useState(false);
    const [autocatchUrl, setAutocatchUrl] = useState("");
    const [sortBy, setSortBy] = useState<"date" | "name" | "size" | "progress">("date");
    const hasAutoResumed = useRef(false);
    const hasStartupReconciled = useRef(false);
    const { settings } = useSettings();

    /**
     * Refreshes the download list and handles the "Auto-Resume" feature.
     * Auto-resume is triggered if the application settings allow it, ensuring 
     * that interrupted downloads restart automatically on app launch.
     */
    const handleRefreshList = useCallback(async () => {
        try {
            const [downloads, settings] = await Promise.all([
                invoke<DownloadItem[]>("get_downloads"),
                invoke<{ auto_resume?: string }>("get_settings")
            ]);
            const hydratedDownloads = downloads.map((d) => ({
                ...d,
                verified_speed: d.protocol === "torrent" ? 0 : d.speed,
                status_text:
                    d.status === "error"
                        ? d.status_text ?? d.error_message ?? "Download failed"
                        : d.status === "downloading"
                            ? d.status_text ?? "Restoring session..."
                        : d.status_text,
                status_phase:
                    d.status === "downloading"
                        ? d.status_phase ?? "restoring_session"
                        : d.status_phase,
                phase_elapsed_secs:
                    d.status === "downloading"
                        ? d.phase_elapsed_secs ?? 0
                        : d.phase_elapsed_secs,
            }));
            setDownloads(hydratedDownloads);

            // Auto-resume should happen once per app load, and sequentially.
            // Flooding resume calls at startup increases contention and can slow
            // torrent initialization when multiple items are marked "downloading".
            if (settings.auto_resume === "true" && !hasAutoResumed.current) {
                hasAutoResumed.current = true;
                for (const d of downloads) {
                    if (d.status === "downloading") {
                        setDownloads((prev) =>
                            prev.map((item) =>
                                item.id === d.id
                                    ? {
                                        ...item,
                                        status: "downloading",
                                        status_text: "Restoring session...",
                                        status_phase: "restoring_session",
                                        phase_elapsed_secs: 0,
                                    }
                                    : item
                            )
                        );
                        await invoke("resume_download", { id: d.id }).catch(console.error);
                        await new Promise((resolve) => setTimeout(resolve, 250));
                    }
                }
            }

            // Reconcile stale "downloading" records once even when auto-resume is disabled.
            // This prevents a reopen state where items look live but are not actually progressing.
            if (!hasStartupReconciled.current && settings.auto_resume !== "true") {
                hasStartupReconciled.current = true;
                const staleActive = downloads.filter((d) => d.status === "downloading");
                for (const d of staleActive) {
                    setDownloads((prev) =>
                        prev.map((item) =>
                            item.id === d.id
                                ? {
                                    ...item,
                                    status: "downloading",
                                    status_text: "Restoring session...",
                                    status_phase: "restoring_session",
                                    phase_elapsed_secs: 0,
                                }
                                : item
                        )
                    );
                    await invoke("resume_download", { id: d.id }).catch(console.error);
                    await new Promise((resolve) => setTimeout(resolve, 250));
                }
            }
        } catch (err) {
            console.error("Failed to fetch downloads:", err);
        }
    }, []);

    /**
     * Side-Effect: IPC Event Listeners
     * 
     * Subscribes to backend events:
     * - `download-progress`: Fine-grained updates for speed, ETA, and bytes.
     * - `download-completed`: Triggers a full list refresh on completion.
     * - `download-name-updated`: Updates filename (e.g., after BitTorrent metadata is fetched).
     * - `autocatch-url`: Detects interesting clipboard links.
     */
    useEffect(() => {
        handleRefreshList();

        const unlistenProgress = listen<ProgressPayload>("download-progress", (event) => {
            const progress = event.payload;
            setDownloads((prev) =>
                prev.map((d) => {
                    if (d.id === progress.id) {
                        // Prevent overwriting completed status with late progress events
                        if (d.status === "completed") return d;

                        return {
                            ...d,
                            downloaded: progress.downloaded,
                            network_received: progress.network_received ?? progress.downloaded,
                            verified_speed: progress.verified_speed ?? progress.speed,
                            size: progress.total,
                            speed: progress.speed,
                            eta: progress.eta,
                            connections: progress.connections,
                            status: progress.status_text === "Paused" || progress.status_phase === "paused" ? "paused" : "downloading",
                            status_text: progress.status_text,
                            status_phase: progress.status_phase,
                            phase_elapsed_secs: progress.phase_elapsed_secs,
                        };
                    }
                    return d;
                })
            );
        });

        const unlistenCompleted = listen<string>("download-completed", async () => {
            handleRefreshList();
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

        // Listen for download errors (e.g., YouTube 403)
        const unlistenError = listen<{ id: string; message: string }>("download-error", (event) => {
            setDownloads((prev) =>
                prev.map((d) =>
                    d.id === event.payload.id
                        ? { ...d, status: "error", status_text: event.payload.message }
                        : d
                )
            );
        });

        return () => {
            unlistenProgress.then((u) => u());
            unlistenCompleted.then((u) => u());
            unlistenName.then((u) => u());
            unlistenAutocatch.then((u) => u());
            unlistenError.then((u) => u());
        };
    }, []);


    // Apply status filter from props
    let processedDownloads = downloads.filter((d) => {
        if (filter === "active") return d.status === "downloading" || d.status === "queued";
        if (filter === "completed") return d.status === "completed";
        if (category && category !== "All") return d.category === category;
        return true;
    });

    // Apply sorting
    processedDownloads = [...processedDownloads].sort((a, b) => {
        switch (sortBy) {
            case "name":
                return a.filename.localeCompare(b.filename);
            case "size":
                return b.size - a.size; // Largest first
            case "progress":
                const progA = a.size > 0 ? a.downloaded / a.size : 0;
                const progB = b.size > 0 ? b.downloaded / b.size : 0;
                return progB - progA; // Most progress first
            case "date":
            default:
                // Assuming id is a timestamp-based UUID or we keep original order
                return 0;
        }
    });

    const filteredDownloads = processedDownloads;

    // Count only truly active downloads for the badge (not completed or errored)
    const activeCount = downloads.filter(d => d.status === "downloading" || d.status === "queued" || d.status === "paused").length;

    return (
        <div className="flex flex-col h-full bg-brand-primary relative overflow-hidden">
            <div className="px-8 pt-5 pb-3 flex items-center justify-between relative z-10">
                <div className="flex flex-col gap-1">
                    <h1 className="text-2xl font-bold text-text-primary tracking-tight flex items-center gap-2">
                        {category ? `${category} Downloads` : filter === "active" ? "Active Downloads" : "All Downloads"}
                        {activeCount > 0 && (
                            <span className="w-4 h-4 flex items-center justify-center rounded-full bg-white text-black shadow-sm text-[9px] font-bold">
                                {activeCount}
                            </span>
                        )}
                    </h1>
                    <p className="text-sm text-text-tertiary">
                        {category ? `Organized collection of ${category.toLowerCase()} files` : "Manage and track all your download tasks"}
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

            {/* Sort Dropdown */}
            <div className="px-8 pb-3 flex items-center justify-end">
                <div className="relative">
                    <select
                        value={sortBy}
                        onChange={(e) => setSortBy(e.target.value as any)}
                        className="appearance-none bg-brand-secondary border border-surface-border rounded-lg pl-3 pr-8 py-1.5 text-xs font-medium text-text-secondary focus:outline-none focus:border-accent-primary transition-colors cursor-pointer"
                    >
                        <option value="date">Date Added</option>
                        <option value="name">Name</option>
                        <option value="size">Size</option>
                        <option value="progress">Progress</option>
                    </select>
                    <ChevronDown size={14} className="absolute right-2 top-1/2 -translate-y-1/2 text-text-tertiary pointer-events-none" />
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
                                setDownloads={setDownloads}
                                showTorrentDebug={settings.torrent_debug_stats}
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


const DownloadCard = memo(React.forwardRef<HTMLDivElement, {
    download: DownloadItem,
    onRefresh: () => void,
    setDownloads: React.Dispatch<React.SetStateAction<DownloadItem[]>>,
    showTorrentDebug: boolean,
}>(
    ({ download, onRefresh, setDownloads, showTorrentDebug }, ref) => {
        const totalBytes = Math.max(download.size, 0);
        const verifiedBytes = Math.max(download.downloaded, 0);
        const [visualProgress, setVisualProgress] = useState(0);
        const [contextMenu, setContextMenu] = useState<{ x: number, y: number } | null>(null);
        const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
        const [deleteFiles, setDeleteFiles] = useState(download.status !== 'completed');
        const [isDeleting, setIsDeleting] = useState(false);
        const statusText = download.status_text ?? "";
        const statusPhase = download.status_phase ?? "";
        const networkReceived = Math.max(download.network_received ?? verifiedBytes, verifiedBytes);
        const isPreparingFirstPiece =
            statusPhase === "preparing_first_piece" ||
            statusText.toLowerCase().includes("preparing first piece");
        const hasNetworkAheadOfVerified = networkReceived > verifiedBytes + 256 * 1024;
        const shouldUseNetworkProgress =
            download.protocol === "torrent" && (isPreparingFirstPiece || hasNetworkAheadOfVerified);
        const displayDownloaded = shouldUseNetworkProgress ? networkReceived : verifiedBytes;
        const progress = totalBytes > 0 ? Math.min((displayDownloaded / totalBytes) * 100, 100) : 0;
        const isIndeterminateStatus =
            statusText === "Initializing..." ||
            statusText === "Fetching Metadata..." ||
            statusText.includes("Restoring") ||
            statusText.includes("Finding peers") ||
            statusText.includes("Connecting") ||
            statusText.includes("Negotiating");
        const shouldRenderStatusLine =
            statusText.includes("Initializing") ||
            statusText.includes("Metadata") ||
            statusText.includes("Restoring") ||
            statusText.includes("Verifying") ||
            statusText.includes("Finding peers") ||
            statusText.includes("Negotiating") ||
            statusText === "Paused" ||
            statusText === "Pausing..." ||
            statusText === "Resuming..." ||
            statusText.includes("Connecting") ||
            statusText.includes("Oops") ||
            isPreparingFirstPiece;
        const verifiedSpeed = download.verified_speed ?? download.speed;
        const shouldShowDualSpeed =
            showTorrentDebug &&
            download.protocol === "torrent" &&
            (isPreparingFirstPiece || hasNetworkAheadOfVerified);

        useEffect(() => {
            setVisualProgress(progress);
        }, [progress]);

        const handlePauseResume = async (e: React.MouseEvent) => {
            e.stopPropagation();
            try {
                if (download.status === "downloading") {
                    setDownloads((prev) =>
                        prev.map((d) =>
                            d.id === download.id
                                ? {
                                    ...d,
                                    status: "paused",
                                    speed: 0,
                                    status_text: "Pausing...",
                                    status_phase: "paused",
                                    phase_elapsed_secs: 0,
                                }
                                : d
                        )
                    );
                    await invoke("pause_download", { id: download.id });
                } else {
                    setDownloads((prev) =>
                        prev.map((d) =>
                            d.id === download.id
                                ? {
                                    ...d,
                                    status: "downloading",
                                    status_text: "Restoring session...",
                                    status_phase: "restoring_session",
                                    phase_elapsed_secs: 0,
                                }
                                : d
                        )
                    );
                    await invoke("resume_download", { id: download.id });
                }
            } catch (err) {
                console.error("Action failed:", err);
                const message = err instanceof Error ? err.message : String(err);
                setDownloads((prev) =>
                    prev.map((d) =>
                        d.id === download.id
                            ? { ...d, status: "error", status_text: message }
                            : d
                    )
                );
                onRefresh();
            }
        };

        const handleDelete = async (e?: React.MouseEvent) => {
            e?.stopPropagation();
            setShowDeleteConfirm(true);
        };

        const performDelete = async () => {
            setIsDeleting(true);
            try {
                await invoke("delete_download", { id: download.id, deleteFiles: deleteFiles });
                setDownloads(prev => prev.filter(d => d.id !== download.id));
                setShowDeleteConfirm(false);
            } catch (err) {
                console.error("Delete failed:", err);
                onRefresh();
            } finally {
                setIsDeleting(false);
            }
        };

        const handleContextMenu = (e: React.MouseEvent) => {
            e.preventDefault();
            setContextMenu({ x: e.clientX, y: e.clientY });
        };

        const getStatusColor = () => {
            if (download.status === 'completed') return 'text-status-success';
            if (download.status === 'error') return 'text-status-error';
            if (download.status === 'paused') return 'text-status-warning';
            return 'text-text-primary';
        };

        const ProtocolIcon = () => {
            return <FileDown className={clsx("w-5 h-5", getStatusColor())} />;
        };

        return (
            <motion.div
                ref={ref}
                layout
                initial={{ opacity: 0, y: 5, scale: 0.99 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, scale: 0.98, transition: { duration: 0.1 } }}
                className="card-base p-4 group card-hover relative overflow-hidden select-none"
                onContextMenu={handleContextMenu}
                onClick={() => setContextMenu(null)}
            >
                <motion.div
                    layout
                    className={clsx(
                        "absolute inset-0 z-0 pointer-events-none transition-colors",
                        isIndeterminateStatus
                            ? "bg-text-primary/10"
                            : "bg-brand-tertiary/20"
                    )}
                    style={{ width: isIndeterminateStatus ? "100%" : `${visualProgress}%` }}
                    animate={isIndeterminateStatus ? {
                        opacity: [0.3, 0.6, 0.3]
                    } : { opacity: 1 }}
                    transition={isIndeterminateStatus ? {
                        duration: 2, repeat: Infinity, ease: "easeInOut"
                    } : { type: "spring", stiffness: 400, damping: 40 }}
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
                                {download.status !== "completed" && (
                                    <button onClick={handlePauseResume} className="btn-ghost p-1.5" title={download.status === "downloading" ? "Pause" : "Resume"}>
                                        {download.status === "downloading" && download.status_text !== "Paused" ? <Pause size={14} /> : <Play size={14} />}
                                    </button>
                                )}
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
                                            (isIndeterminateStatus && totalBytes === 0)
                                                ? 'bg-brand-primary animate-progress-indeterminate bg-[length:1rem_1rem] bg-gradient-to-r from-brand-primary via-brand-secondary to-brand-primary'
                                                : shouldUseNetworkProgress
                                                    ? 'bg-accent/80'
                                                    : 'bg-text-primary'
                                )}
                                style={{ width: `${isIndeterminateStatus && totalBytes === 0 ? 100 : visualProgress}%` }}
                                transition={{ type: "spring", stiffness: 400, damping: 40 }}
                            />
                        </div>

                        <div className="flex items-center justify-between text-xs text-text-secondary mt-0.5">
                            <div className="flex items-center gap-3">
                                {download.status === 'error' && download.status_text ? (
                                    <div className="flex items-center gap-2">
                                        <span className="font-bold uppercase tracking-widest text-[9px] text-status-error">
                                            {download.status_text}
                                        </span>
                                    </div>
                                ) : download.status_text && shouldRenderStatusLine ? (
                                    <div className="flex items-center gap-2">
                                        {download.status_text !== "Paused" && !download.status_text.includes("Oops") && <div className="w-1.5 h-1.5 rounded-full bg-text-primary animate-pulse" />}
                                        <span className={clsx("font-bold uppercase tracking-widest text-[9px]",
                                            download.status_text === "Paused" ? "text-status-warning" :
                                                download.status_text.includes("Oops") ? "text-status-error" : "text-white"
                                        )}>
                                            {download.status_text}
                                        </span>
                                        {download.status_text !== "Paused" && (
                                            <span className="font-mono text-[9px] text-text-tertiary">
                                                {formatPhaseElapsed(download.phase_elapsed_secs)}
                                            </span>
                                        )}
                                        {getPhaseHint(download.status_phase) && (
                                            <span className="text-[9px] uppercase tracking-wider text-text-tertiary">
                                                {getPhaseHint(download.status_phase)}
                                            </span>
                                        )}
                                        {showTorrentDebug && (isPreparingFirstPiece || hasNetworkAheadOfVerified) && (
                                            <span className="text-[9px] uppercase tracking-wider text-text-tertiary">
                                                RX {formatSize(networkReceived)} | Verified {formatSize(download.downloaded)}
                                            </span>
                                        )}
                                    </div>
                                ) : (
                                    <span className="font-medium tracking-wide">
                                        {showTorrentDebug && shouldUseNetworkProgress && (
                                            <span className="text-[9px] uppercase tracking-wider text-text-tertiary mr-1">RX</span>
                                        )}
                                        {formatSize(displayDownloaded)} <span className="text-text-tertiary font-normal px-1">of</span> {formatSize(totalBytes)}
                                    </span>
                                )}

                                {download.status === 'downloading' && download.status_text !== "Paused" && (
                                    <>
                                        <div className="flex items-center gap-1 text-text-primary">
                                            <ArrowDown size={10} />
                                            {shouldShowDualSpeed ? (
                                                <span className="flex items-center gap-1">
                                                    <span className="text-[9px] uppercase tracking-wider text-text-tertiary">RX</span>
                                                    <span>{formatSpeed(download.speed)}</span>
                                                    <span className="text-text-tertiary">|</span>
                                                    <span className="text-[9px] uppercase tracking-wider text-text-tertiary">Verified</span>
                                                    <span>{formatSpeed(verifiedSpeed)}</span>
                                                </span>
                                            ) : (
                                                <span>{formatSpeed(download.speed)}</span>
                                            )}
                                        </div>
                                        <div className="flex items-center gap-1 text-text-tertiary">
                                            {download.protocol === 'torrent' ? <Users size={10} /> : <Wifi size={10} />}
                                            <span>{download.connections}</span>
                                        </div>
                                    </>
                                )}
                            </div>
                            <div className="flex items-center gap-2">
                                {download.status === 'downloading' && download.status_text !== "Paused" && (
                                    <div className="flex items-center gap-1 text-text-tertiary">
                                        <Clock size={10} />
                                        <span className="font-mono text-[10px] tracking-tight">{formatEta(download.eta)} remaining</span>
                                    </div>
                                )}
                                <span className={clsx("font-medium flex items-center gap-1", getStatusColor())}>
                                    {showTorrentDebug && shouldUseNetworkProgress && (
                                        <span className="text-[9px] uppercase tracking-wider text-text-tertiary">RX</span>
                                    )}
                                    {download.status === 'completed' ? 'Done' : `${visualProgress.toFixed(1)}%`}
                                </span>
                            </div>
                        </div>
                    </div>
                </div>

                {/* Context Menu Overlay */}
                <AnimatePresence>
                    {contextMenu && (
                        <>
                            <div className="fixed inset-0 z-40" onClick={() => setContextMenu(null)} onContextMenu={(e) => { e.preventDefault(); setContextMenu(null); }} />
                            <motion.div
                                initial={{ opacity: 0, scale: 0.95 }}
                                animate={{ opacity: 1, scale: 1 }}
                                exit={{ opacity: 0, scale: 0.95 }}
                                style={{ top: contextMenu.y, left: contextMenu.x }}
                                className="fixed z-50 min-w-[180px] bg-brand-primary/95 backdrop-blur-xl border border-brand-tertiary/30 rounded-xl shadow-2xl p-1.5"
                            >
                                <button
                                    onClick={(e) => { setContextMenu(null); handlePauseResume(e); }}
                                    className="w-full text-left flex items-center gap-2 px-3 py-2 text-xs font-medium text-text-primary hover:bg-brand-tertiary/30 rounded-lg transition-colors"
                                >
                                    {download.status === "downloading" ? (
                                        <><Pause size={14} /> Pause</>
                                    ) : (
                                        <><Play size={14} /> Resume</>
                                    )}
                                </button>
                                <button
                                    onClick={() => { setContextMenu(null); invoke("show_in_folder", { path: download.filepath }); }}
                                    className="w-full text-left flex items-center gap-2 px-3 py-2 text-xs font-medium text-text-primary hover:bg-brand-tertiary/30 rounded-lg transition-colors"
                                >
                                    <FolderOpen size={14} />
                                    Open Folder
                                </button>
                                <button
                                    onClick={() => { setContextMenu(null); navigator.clipboard.writeText(download.url); }}
                                    className="w-full text-left flex items-center gap-2 px-3 py-2 text-xs font-medium text-text-primary hover:bg-brand-tertiary/30 rounded-lg transition-colors"
                                >
                                    <Play size={14} className="rotate-45" />
                                    Copy Link
                                </button>
                                <div className="h-px bg-brand-tertiary/20 my-1 mx-1.5" />
                                <button
                                    onClick={() => { setContextMenu(null); handleDelete(); }}
                                    className="w-full text-left flex items-center gap-2 px-3 py-2 text-xs font-medium text-status-error hover:bg-status-error/10 rounded-lg transition-colors"
                                >
                                    <Trash2 size={14} />
                                    Delete
                                </button>
                            </motion.div>
                        </>
                    )}
                </AnimatePresence>

                <ConfirmDialog
                    isOpen={showDeleteConfirm}
                    onClose={() => setShowDeleteConfirm(false)}
                    onConfirm={performDelete}
                    title="Delete Download?"
                    message={`Are you sure you want to remove "${download.filename}" from your history?`}
                    confirmText="Delete"
                    showCheckbox={true}
                    checkboxChecked={deleteFiles}
                    onCheckboxChange={setDeleteFiles}
                    isLoading={isDeleting}
                />
            </motion.div>
        );
    }
));

function AddDownloadModal({ onClose, onAdded, initialUrl = "" }: { onClose: () => void, onAdded: () => void, initialUrl?: string }) {
    const [mode, setMode] = useState<"single" | "batch">("single");
    const [url, setUrl] = useState(initialUrl);
    const [isAdding, setIsAdding] = useState(false);
    const [status, setStatus] = useState<string | null>(null);
    const [torrentInfo, setTorrentInfo] = useState<TorrentInfo | null>(null);
    const [showAdvanced, setShowAdvanced] = useState(false);
    const [userAgent, setUserAgent] = useState("");
    const [cookies, setCookies] = useState("");
    const [startPaused, setStartPaused] = useState(false);
    const { settings } = useSettings();

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
                            if (clipText.includes('\n')) {
                                setMode("batch");
                            }
                        }
                    }
                } catch (e) {
                    // Silent failure
                }
            }
        };
        checkClipboard();
    }, [initialUrl]);

    const getSaveLocation = async () => {
        try {
            const settings = await invoke<Record<string, string>>("get_settings");
            if (settings.ask_location === "true") {
                const selected = await open({
                    directory: true,
                    multiple: false,
                    defaultPath: settings.download_path || undefined,
                });
                return selected as string | null;
            }
        } catch (e) {
            console.error("Failed to check ask_location setting:", e);
        }
        return undefined;
    };

    const handleAdd = async (paused: boolean = false) => {
        if (!url) return;

        const urls = mode === "batch"
            ? url.split('\n').map(u => u.trim()).filter(u => u.length > 0)
            : [url.trim()];

        if (urls.length === 0) return;

        // Batch limit
        const MAX_BATCH_SIZE = 20;
        if (urls.length > MAX_BATCH_SIZE) {
            setStatus(`Maximum ${MAX_BATCH_SIZE} URLs per batch. You have ${urls.length}.`);
            return;
        }

        setIsAdding(true);
        setStartPaused(paused);

        // Single Mode: use the interactive flow (for torrent file selection)
        if (mode === "single") {
            const singleUrl = urls[0];
            try { new URL(singleUrl); } catch (e) {
                // Allow magnet links even if URL parser fails
                if (!singleUrl.startsWith("magnet:")) {
                    setStatus("Error: Invalid URL");
                    setIsAdding(false);
                    return;
                }
            }

            try {
                setStatus("Analyzing...");
                const typeInfo = await invoke<any>("validate_url_type", { url: singleUrl });
                if (typeInfo.is_magnet) {
                    const info = await invoke<TorrentInfo>("analyze_torrent", { url: singleUrl });
                    setTorrentInfo(info);
                    setStatus(null);
                } else {
                    const output_folder = await getSaveLocation();
                    if (output_folder === null) {
                        setIsAdding(false);
                        return;
                    }

                    await invoke("add_download", {
                        url: typeInfo.resolved_url || singleUrl,
                        filename: typeInfo.hinted_filename || "download",
                        filepath: "",
                        outputFolder: output_folder || null,
                        userAgent: userAgent || null,
                        cookies: cookies || null,
                        size: typeInfo.content_length ?? null,
                        startPaused: paused
                    });
                    onAdded();
                    onClose();
                    setIsAdding(false);
                }
            } catch (err) {
                setStatus(`Error: ${err}`);
                setIsAdding(false);
            }
            return;
        }

        // Bulk Mode
        let successCount = 0;
        const output_folder = await getSaveLocation();

        // If user cancels location selection for bulk, abort all
        if (output_folder === null) {
            setIsAdding(false);
            return;
        }

        for (let i = 0; i < urls.length; i++) {
            const currentUrl = urls[i];
            setStatus(`Adding ${i + 1}/${urls.length}...`);

            try {
                const typeInfo = await invoke<any>("validate_url_type", { url: currentUrl });

                if (typeInfo.is_magnet) {
                    // For bulk, we bypass interactive selection and download ALL files (indices: null)
                    await invoke("add_torrent", {
                        url: currentUrl,
                        filename: "Torrent", // Backend will eventually fetch metadata
                        filepath: "",
                        indices: null, // Select all files
                        outputFolder: output_folder || null,
                        startPaused: paused
                    });
                } else {
                    await invoke("add_download", {
                        url: typeInfo.resolved_url || currentUrl,
                        filename: typeInfo.hinted_filename || "download",
                        filepath: "",
                        outputFolder: output_folder || null,
                        userAgent: userAgent || null,
                        cookies: cookies || null,
                        size: typeInfo.content_length ?? null,
                        startPaused: paused
                    });
                }
                successCount++;
            } catch (err) {
                console.error(`Failed to add ${currentUrl}:`, err);
                // Continue with next URL
            }
        }

        setStatus(successCount === urls.length ? "Done!" : `Added ${successCount}/${urls.length} downloads`);
        setTimeout(() => {
            onAdded();
            onClose();
            setIsAdding(false);
        }, 500);
    };

    const handleTorrentSelect = async (indices: number[]) => {
        setIsAdding(true);
        try {
            const output_folder = await getSaveLocation();
            if (output_folder === null) {
                setIsAdding(false);
                return;
            }
            await invoke("add_torrent", {
                url,
                filename: torrentInfo?.name || "Torrent",
                filepath: "",
                indices,
                analysisId: torrentInfo?.id || null,
                totalSize: torrentInfo?.total_size || null,
                outputFolder: output_folder || null,
                startPaused
            });
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
                        <h2 className="text-xl font-bold tracking-tight text-text-primary mb-4">
                            Add New Download
                        </h2>

                        {/* Tabs */}
                        <div className="flex gap-4 mb-4 border-b border-surface-border">
                            <button
                                onClick={() => setMode("single")}
                                className={clsx(
                                    "pb-2 text-sm font-medium transition-colors border-b-2",
                                    mode === "single" ? "text-accent-primary border-accent-primary" : "text-text-secondary border-transparent hover:text-text-primary"
                                )}
                            >
                                Single Link
                            </button>
                            <button
                                onClick={() => setMode("batch")}
                                className={clsx(
                                    "pb-2 text-sm font-medium transition-colors border-b-2",
                                    mode === "batch" ? "text-accent-primary border-accent-primary" : "text-text-secondary border-transparent hover:text-text-primary"
                                )}
                            >
                                Batch List
                            </button>
                        </div>

                        <div className="space-y-4">
                            {mode === "single" ? (
                                <input
                                    autoFocus
                                    type="text"
                                    value={url}
                                    onChange={(e) => setUrl(e.target.value)}
                                    placeholder="https://..."
                                    className="w-full bg-brand-primary border border-surface-border rounded-lg px-4 py-3 text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono text-sm"
                                    onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
                                />
                            ) : (
                                <textarea
                                    autoFocus
                                    value={url}
                                    onChange={(e) => setUrl(e.target.value)}
                                    placeholder="Paste multiple URLs (one per line)..."
                                    className="w-full h-32 bg-brand-primary border border-surface-border rounded-lg px-4 py-3 text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono text-sm resize-none"
                                    onKeyDown={(e) => {
                                        if (e.key === 'Enter' && !e.shiftKey) {
                                            e.preventDefault();
                                            handleAdd();
                                        }
                                    }}
                                />
                            )}

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
                                {settings.scheduler_enabled && (
                                    <button
                                        onClick={() => handleAdd(true)}
                                        disabled={isAdding || !url}
                                        className="btn-secondary text-sm flex items-center gap-2"
                                    >
                                        <Clock size={14} />
                                        <span>Schedule</span>
                                    </button>
                                )}
                                <button onClick={() => handleAdd(false)} disabled={isAdding || !url} className="btn-primary text-sm">
                                    {isAdding ? "Analyzing..." : "Add Download"}
                                </button>
                            </div>
                        </div>
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



