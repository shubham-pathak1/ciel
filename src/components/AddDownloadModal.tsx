import { useEffect, useRef, useState } from "react";
import { AlertCircle, Clock, Database as DatabaseIcon, FileDown, Loader2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { AnimatePresence, motion } from "framer-motion";
import clsx from "clsx";
import { useSettings } from "../hooks/useSettings";
import { ModalPortal } from "./ModalPortal";
import { TorrentFileSelector } from "./TorrentFileSelector";
import type { TorrentInfo } from "../types/downloads";
import { getPathLeafName } from "../utils/downloadFormatting";
import { getFriendlyErrorMessage, isHtmlResponse, isLocalTorrentPath } from "../utils/downloadStatus";
export function AddDownloadModal({ onClose, onAdded, initialUrl = "" }: { onClose: () => void, onAdded: () => void, initialUrl?: string }) {
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
    const analysisStatusTimers = useRef<number[]>([]);
    const analysisRunId = useRef(0);
    const selectedTorrentFile = mode === "single" && isLocalTorrentPath(url) ? getPathLeafName(url) : null;

    const clearAnalysisStatusTimers = () => {
        analysisStatusTimers.current.forEach((timer) => window.clearTimeout(timer));
        analysisStatusTimers.current = [];
    };

    const cancelAnalysisFlow = () => {
        analysisRunId.current += 1;
        clearAnalysisStatusTimers();
        setTorrentInfo(null);
        setStatus(null);
        setIsAdding(false);
    };

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

    useEffect(() => {
        return () => clearAnalysisStatusTimers();
    }, []);

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

    const handleTorrentFileBrowse = async () => {
        try {
            const selected = await open({
                multiple: false,
                filters: [{ name: "Torrent Files", extensions: ["torrent"] }],
            });

            if (typeof selected === "string") {
                setMode("single");
                setUrl(selected);
                setStatus(null);
            }
        } catch (err) {
            setStatus(`Error: ${getFriendlyErrorMessage(String(err))}`);
        }
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
            const isTorrentFile = isLocalTorrentPath(singleUrl);
            try { new URL(singleUrl); } catch (e) {
                // Allow magnet links even if URL parser fails
                if (!singleUrl.startsWith("magnet:") && !isTorrentFile) {
                    setStatus("Error: That link does not look valid. Check it and try again.");
                    setIsAdding(false);
                    return;
                }
            }

            const currentRunId = ++analysisRunId.current;
            try {
                clearAnalysisStatusTimers();
                setStatus("Checking link...");
                let typeInfo: any = null;
                if (!isTorrentFile) {
                    typeInfo = await invoke<any>("validate_url_type", { url: singleUrl });
                    if (analysisRunId.current !== currentRunId) return;
                }

                if (isTorrentFile || typeInfo.is_magnet) {
                    setStatus("Reading torrent metadata...");
                    analysisStatusTimers.current = [
                        window.setTimeout(() => {
                            if (analysisRunId.current === currentRunId) {
                                setStatus("Still reading metadata. Some torrents need more time to find peers.");
                            }
                        }, 8000),
                        window.setTimeout(() => {
                            if (analysisRunId.current === currentRunId) {
                                setStatus("Metadata is taking longer than expected. You can keep waiting, or cancel and try a different source.");
                            }
                        }, 20000),
                    ];
                    const info = await invoke<TorrentInfo>("analyze_torrent", { url: singleUrl });
                    if (analysisRunId.current !== currentRunId) return;
                    clearAnalysisStatusTimers();
                    setTorrentInfo(info);
                    setStatus("Torrent ready. Select files to continue.");
                    setIsAdding(false);
                } else {
                    if (isHtmlResponse(typeInfo)) {
                        setStatus("Error: This server returned a web page instead of the file. It may need login cookies or a browser download link.");
                        setIsAdding(false);
                        return;
                    }

                    if (analysisRunId.current !== currentRunId) return;
                    clearAnalysisStatusTimers();
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
                    setStatus(null);
                    setIsAdding(false);
                }
            } catch (err) {
                if (analysisRunId.current !== currentRunId) return;
                clearAnalysisStatusTimers();
                setStatus(`Error: ${getFriendlyErrorMessage(String(err))}`);
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
            setStatus(`Adding ${i + 1} of ${urls.length}...`);

            try {
                if (isLocalTorrentPath(currentUrl)) {
                    const info = await invoke<TorrentInfo>("analyze_torrent", { url: currentUrl });
                    await invoke("add_torrent", {
                        url: currentUrl,
                        filename: info.name || getPathLeafName(currentUrl),
                        filepath: "",
                        indices: info.files.map((file) => file.index),
                        analysisId: info.id || null,
                        totalSize: info.total_size || null,
                        outputFolder: output_folder || null,
                        startPaused: paused
                    });
                } else {
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
                    } else if (isHtmlResponse(typeInfo)) {
                        console.error(`Skipped ${currentUrl}: server returned an HTML page instead of a file`);
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
        clearAnalysisStatusTimers();
        setIsAdding(true);
        setStatus("Starting torrent...");
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
            setStatus(null);
            onAdded();
            onClose();
        } catch (err) {
            setStatus(`Error: ${getFriendlyErrorMessage(String(err))}`);
        } finally {
            setIsAdding(false);
        }
    };

    const statusMessage = status?.replace(/^Error:\s*/i, "") ?? "";
    const statusIsError = status?.toLowerCase().startsWith("error:");
    const statusIsBusy = Boolean(status && !statusIsError && isAdding);

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
                                <>
                                    <div className="flex gap-2">
                                        <button
                                            type="button"
                                            onClick={() => {
                                                if (selectedTorrentFile) {
                                                    setUrl("");
                                                    setStatus(null);
                                                }
                                            }}
                                            className={clsx(
                                                "flex-1 rounded-lg border px-3 py-2 text-sm transition-colors",
                                                !selectedTorrentFile
                                                    ? "border-text-secondary bg-brand-tertiary text-text-primary"
                                                    : "border-surface-border bg-brand-primary text-text-secondary hover:text-text-primary"
                                            )}
                                        >
                                            Paste Link or Magnet
                                        </button>
                                        <button
                                            type="button"
                                            onClick={handleTorrentFileBrowse}
                                            className={clsx(
                                                "flex-1 rounded-lg border px-3 py-2 text-sm transition-colors inline-flex items-center justify-center gap-2",
                                                selectedTorrentFile
                                                    ? "border-text-secondary bg-brand-tertiary text-text-primary"
                                                    : "border-surface-border bg-brand-primary text-text-secondary hover:text-text-primary"
                                            )}
                                        >
                                            <FileDown size={14} />
                                            Choose .torrent
                                        </button>
                                    </div>

                                    {selectedTorrentFile ? (
                                        <div className="rounded-lg border border-surface-border bg-brand-primary px-4 py-3">
                                            <div className="flex items-center justify-between gap-3">
                                                <div className="min-w-0">
                                                    <div className="text-sm font-medium text-text-primary truncate">
                                                        {selectedTorrentFile}
                                                    </div>
                                                    <div className="text-xs text-text-secondary truncate mt-1">
                                                        Local .torrent file selected
                                                    </div>
                                                </div>
                                                <button
                                                    type="button"
                                                    onClick={handleTorrentFileBrowse}
                                                    className="text-xs text-text-secondary hover:text-text-primary transition-colors shrink-0"
                                                >
                                                    Change
                                                </button>
                                            </div>
                                        </div>
                                    ) : (
                                        <input
                                            autoFocus
                                            type="text"
                                            value={url}
                                            onChange={(e) => setUrl(e.target.value)}
                                            placeholder="https://... or magnet:?"
                                            className="w-full bg-brand-primary border border-surface-border rounded-lg px-4 py-3 text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono text-sm"
                                            onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
                                        />
                                    )}
                                </>
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
                                <div
                                    className={clsx(
                                        "text-xs p-3 rounded-lg border flex items-start gap-2 leading-relaxed",
                                        statusIsError
                                            ? "bg-status-error/10 border-status-error/25 text-status-error"
                                            : "bg-brand-tertiary border-surface-border text-text-secondary"
                                    )}
                                >
                                    {statusIsError ? (
                                        <AlertCircle size={14} className="mt-0.5 shrink-0" />
                                    ) : statusIsBusy ? (
                                        <Loader2 size={14} className="mt-0.5 shrink-0 animate-spin text-text-primary" />
                                    ) : (
                                        <DatabaseIcon size={14} className="mt-0.5 shrink-0" />
                                    )}
                                    <span>{statusMessage}</span>
                                </div>
                            )}
                            <div className="flex justify-end gap-3 mt-8">
                                <button
                                    onClick={() => {
                                        cancelAnalysisFlow();
                                        onClose();
                                    }}
                                    className="px-4 py-2 text-text-secondary text-sm"
                                >
                                    Cancel
                                </button>
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
                                    {isAdding ? "Working..." : "Add Download"}
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
                        onCancel={() => {
                            cancelAnalysisFlow();
                        }}
                    />
                </ModalPortal>
            )}
        </>
    );
}