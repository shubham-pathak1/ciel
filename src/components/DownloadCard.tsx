import React, { memo, useEffect, useState } from "react";
import { AlertCircle, ArrowDown, Clock, FileDown, FolderOpen, Loader2, Pause, Play, Trash2, Users, Wifi } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { AnimatePresence, motion } from "framer-motion";
import clsx from "clsx";
import { ConfirmDialog } from "./ConfirmDialog";
import type { DownloadItem } from "../types/downloads";
import { formatEta, formatPhaseElapsed, formatSize, formatSpeed } from "../utils/downloadFormatting";
import { getFriendlyErrorMessage, getPhaseHint, getTorrentPhaseDisplay } from "../utils/downloadStatus";

export const DownloadCard = memo(React.forwardRef<HTMLDivElement, {
    download: DownloadItem;
    onRefresh: () => void;
    setDownloads: React.Dispatch<React.SetStateAction<DownloadItem[]>>;
}>(
    ({ download, onRefresh, setDownloads }, ref) => {
        const totalBytes = Math.max(download.size, 0);
        const verifiedBytes = totalBytes > 0
            ? Math.min(Math.max(download.downloaded, 0), totalBytes)
            : Math.max(download.downloaded, 0);
        const [visualProgress, setVisualProgress] = useState(0);
        const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
        const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
        const [deleteFiles, setDeleteFiles] = useState(download.status !== "completed");
        const [isDeleting, setIsDeleting] = useState(false);
        const statusText = download.status_text ?? "";
        const statusPhase = download.status_phase ?? "";
        const networkReceivedRaw = Math.max(download.network_received ?? verifiedBytes, verifiedBytes);
        const networkReceived = totalBytes > 0
            ? Math.min(networkReceivedRaw, totalBytes)
            : networkReceivedRaw;
        const isPreparingFirstPiece =
            statusPhase === "preparing_first_piece" ||
            statusText.toLowerCase().includes("preparing first piece");
        const hasNetworkAheadOfVerified = networkReceived > verifiedBytes + 256 * 1024;
        const shouldUseNetworkProgress =
            download.protocol === "torrent" && (isPreparingFirstPiece || hasNetworkAheadOfVerified);
        const isCompleted = download.status === "completed";
        const displayDownloaded = isCompleted
            ? (totalBytes > 0 ? totalBytes : verifiedBytes)
            : (shouldUseNetworkProgress ? networkReceived : verifiedBytes);
        const progress = isCompleted
            ? 100
            : totalBytes > 0
                ? Math.min((displayDownloaded / totalBytes) * 100, 100)
                : 0;
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
            statusPhase === "fallback_single" ||
            statusText === "Paused" ||
            statusText === "Pausing..." ||
            statusText === "Resuming..." ||
            statusText.includes("Connecting") ||
            statusText.includes("Oops") ||
            isPreparingFirstPiece;
        const isHttpNonResumable =
            download.protocol === "http" && download.metadata === "http_no_range";
        const pauseTitle =
            isHttpNonResumable && download.status === "downloading"
                ? "Pause. This server cannot resume, so playing again starts from the beginning."
                : download.status === "downloading"
                    ? "Pause"
                    : "Resume";
        const showLiveConnections =
            download.status === "downloading" &&
            download.status_text !== "Paused" &&
            !["initializing", "connecting", "resuming", "restoring_session"].includes(statusPhase);
        const displayConnections = showLiveConnections ? download.connections : 0;
        const torrentPhaseDisplay =
            download.protocol === "torrent" && download.status !== "error"
                ? getTorrentPhaseDisplay(download)
                : null;
        const friendlyError = getFriendlyErrorMessage(download.status_text ?? download.error_message);

        useEffect(() => {
            setVisualProgress(progress);
        }, [progress]);

        const handlePauseResume = async (e: React.MouseEvent) => {
            e.stopPropagation();
            try {
                if (download.status === "downloading") {
                    setDownloads((prev) =>
                        prev.map((item) =>
                            item.id === download.id
                                ? {
                                    ...item,
                                    status: "paused",
                                    speed: 0,
                                    status_text: "Pausing...",
                                    status_phase: "paused",
                                    phase_elapsed_secs: 0,
                                }
                                : item
                        )
                    );
                    await invoke("pause_download", { id: download.id });
                } else {
                    setDownloads((prev) =>
                        prev.map((item) =>
                            item.id === download.id
                                ? {
                                    ...item,
                                    status: "downloading",
                                    status_text:
                                        item.protocol === "http"
                                            ? (item.metadata === "http_no_range" ? "Restarting..." : "Resuming...")
                                            : "Restoring session...",
                                    status_phase:
                                        item.protocol === "http"
                                            ? (item.metadata === "http_no_range" ? "restarting" : "resuming")
                                            : "restoring_session",
                                    phase_elapsed_secs: 0,
                                    connections: item.protocol === "http" ? 0 : item.connections,
                                    downloaded:
                                        item.protocol === "http" && item.metadata === "http_no_range"
                                            ? 0
                                            : item.downloaded,
                                }
                                : item
                        )
                    );
                    await invoke("resume_download", { id: download.id });
                }
            } catch (err) {
                console.error("Action failed:", err);
                const message = err instanceof Error ? err.message : String(err);
                setDownloads((prev) =>
                    prev.map((item) =>
                        item.id === download.id
                            ? { ...item, status: "error", status_text: message }
                            : item
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
                await invoke("delete_download", { id: download.id, deleteFiles });
                setDownloads((prev) => prev.filter((item) => item.id !== download.id));
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
            if (download.status === "completed") return "text-status-success";
            if (download.status === "error") return "text-status-error";
            if (download.status === "paused") return "text-status-warning";
            return "text-text-primary";
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
                        opacity: [0.3, 0.6, 0.3],
                    } : { opacity: 1 }}
                    transition={isIndeterminateStatus ? {
                        duration: 2, repeat: Infinity, ease: "easeInOut",
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
                                    <button onClick={handlePauseResume} className="btn-ghost p-1.5" title={pauseTitle}>
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
                                    download.status === "completed" ? "bg-status-success" :
                                        download.status === "error" ? "bg-status-error" :
                                            (isIndeterminateStatus && totalBytes === 0)
                                                ? "bg-brand-primary animate-progress-indeterminate bg-[length:1rem_1rem] bg-gradient-to-r from-brand-primary via-brand-secondary to-brand-primary"
                                                : shouldUseNetworkProgress
                                                    ? "bg-accent/80"
                                                    : "bg-text-primary"
                                )}
                                style={{ width: `${isIndeterminateStatus && totalBytes === 0 ? 100 : visualProgress}%` }}
                                transition={{ type: "spring", stiffness: 400, damping: 40 }}
                            />
                        </div>

                        <div className="flex items-center justify-between text-xs text-text-secondary mt-0.5">
                            <div className="flex items-center gap-3">
                                {download.status === "error" ? (
                                    <div className="flex items-center gap-2 max-w-[32rem]" title={download.status_text ?? download.error_message ?? undefined}>
                                        <AlertCircle size={12} className="text-status-error shrink-0" />
                                        <span className="font-medium text-[11px] text-status-error truncate">
                                            {friendlyError}
                                        </span>
                                    </div>
                                ) : torrentPhaseDisplay ? (
                                    <div className="flex items-center gap-2 min-w-0">
                                        <Loader2 size={12} className="text-text-primary animate-spin shrink-0" />
                                        <span className="font-semibold text-[10px] uppercase tracking-widest text-white">
                                            {torrentPhaseDisplay.title}
                                        </span>
                                        <span className="text-[10px] text-text-tertiary truncate">
                                            {torrentPhaseDisplay.detail}
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
                                    </div>
                                ) : (
                                    <div className="flex items-center gap-2 font-medium tracking-wide">
                                        <span>
                                            {formatSize(displayDownloaded)} <span className="text-text-tertiary font-normal px-1">of</span> {formatSize(totalBytes)}
                                        </span>
                                    </div>
                                )}

                                {download.status === "downloading" && download.status_text !== "Paused" && (
                                    <>
                                        <div className="flex items-center gap-1 text-text-primary">
                                            <ArrowDown size={10} />
                                            <span>{formatSpeed(download.speed)}</span>
                                        </div>
                                        <div className="flex items-center gap-1 text-text-tertiary">
                                            {download.protocol === "torrent" ? <Users size={10} /> : <Wifi size={10} />}
                                            <span>{displayConnections}</span>
                                        </div>
                                    </>
                                )}
                            </div>
                            <div className="flex items-center gap-2">
                                {download.status === "downloading" && download.status_text !== "Paused" && (
                                    <div className="flex items-center gap-1 text-text-tertiary">
                                        <Clock size={10} />
                                        <span className="font-mono text-[10px] tracking-tight">{formatEta(download.eta)} remaining</span>
                                    </div>
                                )}
                                <span className={clsx("font-medium flex items-center gap-1", getStatusColor())}>
                                    {download.status === "completed" ? "Done" : `${visualProgress.toFixed(1)}%`}
                                </span>
                            </div>
                        </div>

                        {isHttpNonResumable && download.status !== "completed" && download.status !== "error" && (
                            <div className="mt-1.5 flex items-center gap-1.5 text-[10px] font-medium text-text-secondary">
                                <AlertCircle size={11} className="text-status-warning/80 shrink-0" />
                                <span>This server cannot resume. Pausing will restart this file.</span>
                            </div>
                        )}
                    </div>
                </div>

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
