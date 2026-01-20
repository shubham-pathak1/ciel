/**
 * @file TorrentFileSelector.tsx
 * @description A modal component that allows users to pick specific files from a torrent 
 * for selective downloading.
 */

import React, { useState, useMemo } from 'react';
import { X, Check, File, Database } from 'lucide-react';
import { clsx } from 'clsx';
import { motion } from 'framer-motion';

/**
 * Metadata for a single file within a Torrent.
 */
interface TorrentFile {
    name: string;
    size: number;
    index: number;
}

/**
 * Summary of a BitTorrent archive before it is added to the active session.
 */
interface TorrentInfo {
    name: string;
    total_size: number;
    files: TorrentFile[];
}

interface Props {
    info: TorrentInfo;
    onSelect: (indices: number[]) => void;
    onCancel: () => void;
}

/**
 * TorrentFileSelector Component.
 * 
 * Responsibilities:
 * - Renders a hierarchical or flat list of files found in a `.torrent` or magnet metadata.
 * - Manages a selection set of file indices.
 * - Calculates the real-time total size of the selected files.
 * - Triggers the actual download start via `onSelect`.
 */
export const TorrentFileSelector: React.FC<Props> = ({ info, onSelect, onCancel }) => {
    const [selectedIndices, setSelectedIndices] = useState<Set<number>>(
        new Set(info.files.map(f => f.index))
    );

    const toggleFile = (index: number) => {
        const next = new Set(selectedIndices);
        if (next.has(index)) {
            next.delete(index);
        } else {
            next.add(index);
        }
        setSelectedIndices(next);
    };

    const toggleAll = () => {
        if (selectedIndices.size === info.files.length) {
            setSelectedIndices(new Set());
        } else {
            setSelectedIndices(new Set(info.files.map(f => f.index)));
        }
    };

    const formatSize = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const selectedSize = useMemo(() => {
        return info.files
            .filter(f => selectedIndices.has(f.index))
            .reduce((acc, f) => acc + f.size, 0);
    }, [selectedIndices, info.files]);

    return (
        <div className="fixed inset-0 z-[110] flex items-center justify-center p-6 bg-black/60 backdrop-blur-md text-left">
            <motion.div
                initial={{ opacity: 0, scale: 0.98, y: 10 }}
                animate={{ opacity: 1, scale: 1, y: 0 }}
                exit={{ opacity: 0, scale: 0.98, y: 5 }}
                transition={{ duration: 0.15, ease: "easeOut" }}
                className="bg-brand-secondary border border-surface-border w-full max-w-2xl rounded-xl shadow-2xl flex flex-col max-h-[80vh] overflow-hidden"
            >
                {/* Header */}
                <div className="p-4 border-b border-surface-border flex items-center justify-between bg-surface-primary/30">
                    <div>
                        <h3 className="text-lg font-semibold text-text-primary flex items-center gap-2">
                            <Database size={18} className="text-text-tertiary" />
                            Select Files
                        </h3>
                        <p className="text-xs text-text-secondary truncate max-w-md mt-0.5">
                            {info.name}
                        </p>
                    </div>
                    <button
                        onClick={onCancel}
                        className="p-1.5 hover:bg-surface-hover rounded-lg transition-colors text-text-tertiary hover:text-text-primary"
                    >
                        <X size={18} />
                    </button>
                </div>

                {/* File List */}
                <div className="flex-1 overflow-y-auto p-2 space-y-1">
                    <div
                        className="flex items-center gap-3 p-3 rounded-lg hover:bg-surface-hover transition-colors cursor-pointer group"
                        onClick={toggleAll}
                    >
                        <div className={clsx(
                            "w-4 h-4 rounded border flex items-center justify-center transition-all",
                            selectedIndices.size === info.files.length ? "bg-text-primary border-text-primary" : "border-surface-border"
                        )}>
                            {selectedIndices.size === info.files.length && <Check size={12} className="text-brand-primary" />}
                            {selectedIndices.size > 0 && selectedIndices.size < info.files.length && (
                                <div className="w-2 h-0.5 bg-text-primary rounded-full" />
                            )}
                        </div>
                        <span className="text-sm font-medium text-text-primary">Select All</span>
                        <span className="text-xs text-text-tertiary ml-auto">
                            {selectedIndices.size} of {info.files.length} files
                        </span>
                    </div>

                    <div className="h-px bg-surface-border my-1 mx-2" />

                    {info.files.map((file) => (
                        <div
                            key={file.index}
                            className="flex items-center gap-3 p-3 rounded-lg hover:bg-surface-hover transition-colors cursor-pointer group"
                            onClick={() => toggleFile(file.index)}
                        >
                            <div className={clsx(
                                "w-4 h-4 rounded border flex items-center justify-center transition-all",
                                selectedIndices.has(file.index) ? "bg-text-primary border-text-primary" : "border-surface-border"
                            )}>
                                {selectedIndices.has(file.index) && <Check size={12} className="text-brand-primary" />}
                            </div>
                            <File size={16} className="text-text-tertiary shrink-0" />
                            <span className="text-sm text-text-secondary truncate group-hover:text-text-primary transition-colors">
                                {file.name}
                            </span>
                            <span className="text-xs font-mono text-text-tertiary ml-auto shrink-0">
                                {formatSize(file.size)}
                            </span>
                        </div>
                    ))}
                </div>

                {/* Footer */}
                <div className="p-4 border-t border-surface-border bg-surface-primary/30 flex items-center justify-between">
                    <div className="flex flex-col">
                        <span className="text-xs text-text-tertiary uppercase tracking-wider font-semibold">Total Selected</span>
                        <span className="text-sm font-mono text-text-primary">{formatSize(selectedSize)}</span>
                    </div>
                    <div className="flex items-center gap-3">
                        <button
                            onClick={onCancel}
                            className="btn-ghost px-4 py-2 text-sm font-medium"
                        >
                            Cancel
                        </button>
                        <button
                            onClick={() => onSelect(Array.from(selectedIndices))}
                            disabled={selectedIndices.size === 0}
                            className="btn-primary px-6 py-2 text-sm font-bold disabled:opacity-50 disabled:cursor-not-allowed shadow-lg shadow-brand-primary/10"
                        >
                            Start Download
                        </button>
                    </div>
                </div>
            </motion.div>
        </div>
    );
};
