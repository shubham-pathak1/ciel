import { useState, useEffect, useCallback } from 'react';
import { Clock, Play, Pause, Timer, Zap, Info, FileText } from 'lucide-react';
import { useSettings } from '../hooks/useSettings';
import { motion } from 'framer-motion';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

interface DownloadItem {
    id: string;
    filename: string;
    size: number;
    downloaded: number;
    status: string;
    protocol: "http" | "torrent";
}

export function Scheduler() {
    const { settings, updateSetting } = useSettings();
    const [currentTime, setCurrentTime] = useState(new Date());
    const [statusMessage, setStatusMessage] = useState("");
    const [nextEvent, setNextEvent] = useState<{ type: 'start' | 'pause', time: string } | null>(null);
    const [pendingTasks, setPendingTasks] = useState<DownloadItem[]>([]);

    useEffect(() => {
        const timer = setInterval(() => setCurrentTime(new Date()), 1000);
        return () => clearInterval(timer);
    }, []);

    const fetchDownloads = useCallback(async () => {
        try {
            const all = await invoke<DownloadItem[]>("get_downloads");
            const paused = all.filter(d => d.status.toLowerCase() === 'paused');
            setPendingTasks(paused);
        } catch (err) {
            console.error("Failed to fetch downloads for scheduler:", err);
        }
    }, []);

    useEffect(() => {
        calculateNextEvent();
    }, [settings.scheduler_start_time, settings.scheduler_pause_time, currentTime]);

    useEffect(() => {
        fetchDownloads();

        // Listen for new items or status changes
        const unlisten = listen("download-completed", () => fetchDownloads());
        const refreshInterval = setInterval(fetchDownloads, 10000); // 10s fallback refresh

        return () => {
            unlisten.then(f => f());
            clearInterval(refreshInterval);
        };
    }, [fetchDownloads]);

    const calculateNextEvent = () => {
        const now = currentTime.getHours() * 60 + currentTime.getMinutes();

        const [startH, startM] = settings.scheduler_start_time.split(':').map(Number);
        const [pauseH, pauseM] = settings.scheduler_pause_time.split(':').map(Number);

        const startTotal = startH * 60 + startM;
        const pauseTotal = pauseH * 60 + pauseM;

        // Simple logic for next event
        let nextType: 'start' | 'pause' = 'start';
        let nextTime = "";
        let diffMinutes = 0;

        // If currently in active window (Start < Now < Pause)
        const isActiveNow = startTotal < pauseTotal
            ? (now >= startTotal && now < pauseTotal)
            : (now >= startTotal || now < pauseTotal);

        if (isActiveNow) {
            nextType = 'pause';
            nextTime = settings.scheduler_pause_time;
            diffMinutes = pauseTotal > now ? (pauseTotal - now) : (1440 - now + pauseTotal);
        } else {
            nextType = 'start';
            nextTime = settings.scheduler_start_time;
            diffMinutes = startTotal > now ? (startTotal - now) : (1440 - now + startTotal);
        }

        const h = Math.floor(diffMinutes / 60);
        const m = diffMinutes % 60;

        setNextEvent({ type: nextType, time: nextTime });
        setStatusMessage(h > 0 ? `${h}h ${m}m remaining` : `${m}m remaining`);
    };

    const formatSize = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
    };

    return (
        <div className="min-h-full flex flex-col w-full max-w-5xl mx-auto space-y-8 pb-10 animate-in fade-in duration-500">
            {/* Header */}
            <div>
                <h1 className="text-2xl font-semibold text-text-primary tracking-tight mb-1">Download Scheduler</h1>
                <p className="text-sm text-text-secondary">Automate your bandwidth based on local time windows</p>
            </div>

            {/* Status Card */}
            <div className="relative overflow-hidden bg-brand-secondary border border-surface-border rounded-2xl p-6 shadow-xl group">
                <div className="absolute top-0 right-0 p-4 opacity-10 group-hover:opacity-20 transition-opacity">
                    <Clock size={120} />
                </div>

                <div className="relative z-10 flex flex-col md:flex-row md:items-center justify-between gap-6">
                    <div className="space-y-1">
                        <div className="flex items-center gap-2 text-brand-accent text-sm font-bold uppercase tracking-widest">
                            <Timer size={14} />
                            <span>Next Event</span>
                        </div>
                        <h2 className="text-3xl font-bold text-text-primary">
                            {nextEvent?.type === 'start' ? 'Resume' : 'Pause'} at <span className="text-white">{nextEvent?.time}</span>
                        </h2>
                        <p className="text-text-tertiary text-sm font-medium">
                            {statusMessage} • Based on your local system clock
                        </p>
                    </div>

                    <div className="flex items-center gap-3">
                        <div className={`px-4 py-2 rounded-full text-xs font-bold uppercase tracking-wider flex items-center gap-2 ${nextEvent?.type === 'pause' ? 'bg-brand-accent/10 text-brand-accent border border-brand-accent/20' : 'bg-text-tertiary/10 text-text-tertiary border border-surface-border'}`}>
                            <div className={`w-1.5 h-1.5 rounded-full ${nextEvent?.type === 'pause' ? 'bg-brand-accent animate-pulse' : 'bg-text-tertiary'}`} />
                            {nextEvent?.type === 'pause' ? 'Currently Active' : 'Idle'}
                        </div>
                    </div>
                </div>

                {/* Timeline Progress Mockup */}
                <div className="mt-8 h-1.5 w-full bg-brand-tertiary rounded-full overflow-hidden">
                    <motion.div
                        initial={{ width: 0 }}
                        animate={{ width: nextEvent?.type === 'pause' ? '100%' : '0%' }}
                        className="h-full bg-gradient-to-r from-brand-accent to-blue-400"
                    />
                </div>
            </div>

            {/* Configuration Workspace */}
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <div className="bg-brand-secondary border border-surface-border rounded-2xl p-6 space-y-4 hover:border-text-primary/10 transition-colors">
                    <div className="flex items-center gap-3 mb-2">
                        <div className="p-2 rounded-lg bg-text-primary/5 text-text-primary">
                            <Play size={20} fill="currentColor" />
                        </div>
                        <div>
                            <h3 className="font-bold text-text-primary">Resume Downloads</h3>
                            <p className="text-xs text-text-tertiary uppercase tracking-tighter">Automatic Start Trigger</p>
                        </div>
                    </div>
                    <input
                        type="time"
                        value={settings.scheduler_start_time}
                        onChange={(e) => updateSetting('scheduler_start_time', e.target.value)}
                        className="w-full bg-brand-tertiary border border-surface-border rounded-xl px-4 py-3 text-lg font-mono text-text-primary outline-none focus:border-brand-accent transition-all [color-scheme:dark]"
                    />
                    <p className="text-xs text-text-tertiary leading-relaxed">
                        Ciel will automatically wake up all paused and queued downloads at this time.
                    </p>
                </div>

                <div className="bg-brand-secondary border border-surface-border rounded-2xl p-6 space-y-4 hover:border-text-primary/10 transition-colors">
                    <div className="flex items-center gap-3 mb-2">
                        <div className="p-2 rounded-lg bg-text-primary/5 text-text-primary">
                            <Pause size={20} fill="currentColor" />
                        </div>
                        <div>
                            <h3 className="font-bold text-text-primary">Pause Downloads</h3>
                            <p className="text-xs text-text-tertiary uppercase tracking-tighter">Automatic Halt Trigger</p>
                        </div>
                    </div>
                    <input
                        type="time"
                        value={settings.scheduler_pause_time}
                        onChange={(e) => updateSetting('scheduler_pause_time', e.target.value)}
                        className="w-full bg-brand-tertiary border border-surface-border rounded-xl px-4 py-3 text-lg font-mono text-text-primary outline-none focus:border-brand-accent transition-all [color-scheme:dark]"
                    />
                    <p className="text-xs text-text-tertiary leading-relaxed">
                        All active transfers will be safely suspended to preserve bandwidth for other activities.
                    </p>
                </div>
            </div>

            {/* Pending Downloads List */}
            <div className="bg-brand-secondary border border-surface-border rounded-2xl overflow-hidden">
                <div className="p-6 border-b border-surface-border flex items-center justify-between">
                    <div className="flex items-center gap-3">
                        <div className="p-2 rounded-lg bg-brand-accent/10 text-brand-accent">
                            <Clock size={20} />
                        </div>
                        <div>
                            <h3 className="font-bold text-text-primary">Waiting for Trigger</h3>
                            <p className="text-xs text-text-tertiary uppercase tracking-tighter">{pendingTasks.length} {pendingTasks.length === 1 ? 'Task' : 'Tasks'} Queued</p>
                        </div>
                    </div>

                    {pendingTasks.some(t => t.protocol === 'http') && (
                        <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-orange-500/10 text-orange-400 text-[10px] font-bold uppercase tracking-wider border border-orange-500/20">
                            <Zap size={10} />
                            <span>HTTP Links may expire</span>
                        </div>
                    )}
                </div>

                <div className="divide-y divide-surface-border/50 max-h-[300px] overflow-y-auto">
                    {pendingTasks.length > 0 ? (
                        pendingTasks.map((task) => (
                            <div key={task.id} className="p-4 flex items-center justify-between hover:bg-white/5 transition-colors group">
                                <div className="flex items-center gap-4">
                                    <div className="p-2 rounded-lg bg-brand-tertiary text-text-secondary group-hover:text-text-primary transition-colors">
                                        <FileText size={18} />
                                    </div>
                                    <div className="space-y-1">
                                        <p className="text-sm font-medium text-text-primary line-clamp-1">{task.filename}</p>
                                        <div className="flex items-center gap-3">
                                            <span className="text-[10px] text-text-tertiary font-mono uppercase tracking-widest px-1.5 py-0.5 rounded border border-surface-border bg-brand-tertiary">
                                                {task.protocol}
                                            </span>
                                            <span className="text-[10px] text-text-tertiary">
                                                {formatSize(task.size)}
                                            </span>

                                            {task.protocol === 'http' && (
                                                <div className="flex items-center gap-1 text-[10px] text-orange-400 underline decoration-orange-400/30 underline-offset-2">
                                                    <Info size={10} />
                                                    <span>Temporary Link Warning</span>
                                                </div>
                                            )}
                                        </div>
                                    </div>
                                </div>

                                <div className="flex items-center gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                                    <span className="text-[10px] text-text-tertiary italic">Awaiting scheduling...</span>
                                </div>
                            </div>
                        ))
                    ) : (
                        <div className="p-10 flex flex-col items-center justify-center text-center space-y-3">
                            <div className="p-4 rounded-full bg-brand-tertiary/50 text-text-tertiary">
                                <Clock size={32} strokeWidth={1.5} />
                            </div>
                            <div className="space-y-1">
                                <p className="text-sm font-medium text-text-secondary">Queue is empty</p>
                                <p className="text-[10px] text-text-tertiary max-w-[200px]">
                                    Downloads added using 'Schedule for Later' will appear here.
                                </p>
                            </div>
                        </div>
                    )}
                </div>
            </div>

        </div>
    );
}
