/**
 * @file History.tsx
 * @description Provides a searchable view of completed download tasks stored in the persistence layer.
 */

import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Search, Trash2, ExternalLink, Calendar, Database, File } from 'lucide-react';
import clsx from 'clsx';

/**
 * Represents a historical download record.
 */
interface HistoryItem {
    id: string;
    filename: string;
    url: string;
    size: number;
    filepath: string;
    protocol: 'http' | 'torrent';
    completed_at: string;
}

/**
 * History Component.
 * 
 * Responsibilities:
 * - Fetches historical records from the backend database via `get_history`.
 * - Provides client-side filtering (search) by filename or URL.
 * - Allows users to locate files on disk or remove records from the archive.
 */
export const History: React.FC = () => {
    const [history, setHistory] = useState<HistoryItem[]>([]);
    const [search, setSearch] = useState('');
    const [isLoading, setIsLoading] = useState(true);

    useEffect(() => {
        fetchHistory();
    }, []);

    const fetchHistory = async () => {
        setIsLoading(true);
        try {
            const result = await invoke<HistoryItem[]>("get_history");
            setHistory(result);
        } catch (err) {
            console.error("Failed to fetch history:", err);
        } finally {
            setIsLoading(false);
        }
    };

    const handleDelete = async (id: string, e: React.MouseEvent) => {
        e.stopPropagation();
        try {
            await invoke("delete_download", { id });
            setHistory(prev => prev.filter(item => item.id !== id));
        } catch (err) {
            console.error("Failed to delete history item:", err);
        }
    };

    const formatSize = (bytes: number) => {
        if (bytes === 0) return "0 B";
        const k = 1024;
        const sizes = ["B", "KB", "MB", "GB", "TB"];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + " " + sizes[i];
    };

    const formatDate = (dateStr: string) => {
        const date = new Date(dateStr);
        return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' }) +
            ' • ' +
            date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    };

    const filteredHistory = history.filter(item =>
        item.filename.toLowerCase().includes(search.toLowerCase()) ||
        item.url.toLowerCase().includes(search.toLowerCase())
    );

    return (
        <div className="h-full flex flex-col w-full max-w-5xl mx-auto">
            {/* Header */}
            <div className="flex items-end justify-between mb-8 sticky top-0 bg-brand-primary z-20 py-4 border-b border-transparent">
                <div>
                    <h1 className="text-2xl font-semibold text-text-primary tracking-tight mb-1">History</h1>
                    <p className="text-sm text-text-secondary">
                        {filteredHistory.length} {filteredHistory.length === 1 ? 'item' : 'items'} in your archive
                    </p>
                </div>

                <div className="relative group w-72">
                    <div className="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
                        <Search className="h-4 w-4 text-text-tertiary group-focus-within:text-text-primary transition-colors" />
                    </div>
                    <input
                        type="text"
                        placeholder="Search filenames or URLs..."
                        value={search}
                        onChange={(e) => setSearch(e.target.value)}
                        className="block w-full pl-10 pr-3 py-2 border border-surface-border rounded-lg leading-5 bg-brand-secondary text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-text-secondary transition-all text-sm"
                    />
                </div>
            </div>

            {/* List */}
            <div className="flex-1 card-base p-4 relative overflow-hidden flex flex-col">
                <div className="flex-1 overflow-y-auto scrollbar-hide p-2 relative z-10">
                    {isLoading ? (
                        <div className="flex flex-col items-center justify-center h-full gap-4">
                            <div className="w-8 h-8 border-2 border-text-primary border-t-transparent rounded-full animate-spin" />
                            <p className="text-sm text-text-tertiary font-medium">Loading archive...</p>
                        </div>
                    ) : filteredHistory.length === 0 ? (
                        <div className="flex flex-col items-center justify-center h-full text-text-tertiary gap-4 opacity-70">
                            <div className="w-16 h-16 rounded-2xl bg-brand-secondary border border-surface-border flex items-center justify-center">
                                <Database className="w-8 h-8 stroke-[1.5]" />
                            </div>
                            <p className="text-base font-medium">{search ? 'No matches found' : 'History is empty'}</p>
                        </div>
                    ) : (
                        <div className="space-y-2">
                            {filteredHistory.map((item) => (
                                <div
                                    key={item.id}
                                    className="group relative p-3 rounded-xl bg-brand-primary border border-surface-border hover:border-text-tertiary transition-all duration-200"
                                >
                                    <div className="flex items-start gap-3">
                                        {/* Icon */}
                                        <div className={clsx(
                                            "w-10 h-10 rounded-lg flex items-center justify-center border transition-colors",
                                            item.protocol === 'torrent'
                                                ? "bg-brand-secondary border-surface-border text-text-secondary"
                                                : "bg-brand-secondary border-surface-border text-text-primary"
                                        )}>
                                            <File size={18} />
                                        </div>

                                        {/* Content */}
                                        <div className="flex-1 min-w-0 pt-0.5">
                                            <div className="flex items-center justify-between gap-4 mb-0.5">
                                                <h3 className="text-sm font-medium text-text-primary truncate pr-4 group-hover:text-text-primary transition-colors">
                                                    {item.filename}
                                                </h3>
                                                <span className="text-xs text-text-tertiary font-mono whitespace-nowrap hidden sm:block">
                                                    ID: {item.id.slice(0, 8)}
                                                </span>
                                            </div>

                                            <p className="text-xs text-text-tertiary truncate max-w-lg mb-2 cursor-text select-all">
                                                {item.url}
                                            </p>

                                            <div className="flex items-center gap-4">
                                                <div className="flex items-center gap-1.5 text-xs text-text-secondary">
                                                    <Calendar className="w-3.5 h-3.5" />
                                                    {formatDate(item.completed_at || new Date().toISOString())}
                                                </div>
                                                <div className="flex items-center gap-1.5 text-xs text-text-secondary">
                                                    <Database className="w-3.5 h-3.5" />
                                                    {formatSize(item.size)}
                                                </div>
                                            </div>
                                        </div>

                                        {/* Actions */}
                                        <div className="flex items-center gap-2 opacity-0 group-hover:opacity-100 transition-all translate-x-2 group-hover:translate-x-0 self-center">
                                            <button
                                                onClick={() => invoke("show_in_folder", { path: item.filepath })}
                                                title="Open Location"
                                                className="p-2 rounded-lg hover:bg-brand-tertiary text-text-secondary hover:text-text-primary transition-all"
                                            >
                                                <ExternalLink className="w-4 h-4" />
                                            </button>
                                            <button
                                                onClick={(e) => handleDelete(item.id, e)}
                                                title="Delete Record"
                                                className="p-2 rounded-lg hover:bg-brand-tertiary text-text-secondary hover:text-status-error transition-all"
                                            >
                                                <Trash2 className="w-4 h-4" />
                                            </button>
                                        </div>
                                    </div>
                                </div>
                            ))}
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
};
