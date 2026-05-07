/**
 * @file DownloadQueue.tsx
 * @description The primary view for managing the download queue. 
 * Orchestrates HTTP, Torrent, and Video downloads by communicating with the Tauri backend.
 */

import { useState } from "react";
import { CloudDownload, FileDown, ArrowDown, ChevronDown } from "lucide-react";
import { AnimatePresence } from "framer-motion";
import { useDownloads } from "../hooks/useDownloads";
import { AddDownloadModal } from "./AddDownloadModal";
import { DownloadCard } from "./DownloadCard";

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
 * Main Download Queue Component.
 * 
 * Responsibilities:
 * - Fetches initial download list from the backend.
 * - Listens for real-time progress events over the Tauri IPC bridge.
 * - Handles user interactions for pausing, resuming, and deleting downloads.
 * - Manages "Auto-Catch" modal triggers when interesting URLs are found in the clipboard.
 */
export function DownloadQueue({ filter, category }: DownloadQueueProps) {
    const [isAddModalOpen, setIsAddModalOpen] = useState(false);
    const [sortBy, setSortBy] = useState<"date" | "name" | "size" | "progress">("date");
    const { autocatchUrl, downloads, refreshDownloads, setDownloads } = useDownloads();


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
                        <span>Add Download</span>
                    </button>
                    <button
                        onClick={refreshDownloads}
                        className="btn-secondary p-2.5"
                        title="Refresh"
                    >
                        <ArrowDown size={18} className="text-text-secondary" />
                    </button>
                </div>
            </div>

            {/* Sort Dropdown */}
            {filteredDownloads.length > 0 && (
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
            )}

            {/* Download List */}
            {filteredDownloads.length === 0 ? (
                <EmptyState filter={filter} />
            ) : (
                <div className="flex-1 space-y-3 overflow-y-auto pr-2 pb-12 scrollbar-hide">
                    <AnimatePresence mode="popLayout" initial={false}>
                        {filteredDownloads.map((download) => (
                            <DownloadCard
                                key={download.id}
                                download={download}
                                onRefresh={refreshDownloads}
                                setDownloads={setDownloads}
                            />
                        ))}
                    </AnimatePresence>
                </div>
            )}

            <AnimatePresence>
                {isAddModalOpen && (
                    <AddDownloadModal
                        onClose={() => setIsAddModalOpen(false)}
                        onAdded={refreshDownloads}
                        initialUrl={autocatchUrl}
                    />
                )}
            </AnimatePresence>

            {/* Removed AutocatchNotification */}
        </div>
    );
}

function EmptyState({ filter }: { filter: string }) {
    return (
        <div className="flex-1 flex flex-col items-center justify-center text-center p-12">
            <div className="w-24 h-24 rounded-full bg-brand-secondary border border-surface-border flex items-center justify-center mb-6">
                <CloudDownload size={32} className="text-text-tertiary" />
            </div>
            <h2 className="text-xl font-medium text-text-primary mb-2">No active downloads</h2>
            <p className="text-text-secondary max-w-sm mb-8">
                {filter === "active"
                    ? "Your queue is empty."
                    : "Add a link, magnet, or .torrent file from the button above."}
            </p>
        </div>
    );
}
