/**
 * @file Settings.tsx
 * @description Centralized configuration management for the application.
 * Handles persistence of user preferences through the Tauri IPC bridge.
 */

import { useEffect, useState } from "react";
import logo from "../assets/logo.png";
import { Folder, Globe, Gauge, Shield, Info, Check, Save, AlertTriangle, Github, FileText, Cpu, Clock } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import clsx from "clsx";
import { open } from "@tauri-apps/plugin-dialog";
import { motion } from "framer-motion";
import { useSettings, SettingsState } from "../hooks/useSettings";
import { useUpdater } from "../hooks/useUpdater";


/**
 * Settings Component.
 * 
 * Responsibilities:
 * - Loads all preferences from the backend on mount.
 * - Categorizes settings into logical sections (General, Network, Performance, etc.).
 * - Sanitizes and serializes boolean/string values for the SQLite storage layer.
 * - Provides a visual "Save" feedback loop.
 */
export function Settings() {
    const { settings, saveAll } = useSettings();
    const [localSettings, setLocalSettings] = useState<SettingsState>(settings);
    const [activeSection, setActiveSection] = useState("general");
    const [isSaving, setIsSaving] = useState(false);
    const [showSuccess, setShowSuccess] = useState(false);
    const [hasActiveDownloads, setHasActiveDownloads] = useState(false);
    const { checkForUpdates, checking } = useUpdater();

    useEffect(() => {
        setLocalSettings(settings);
    }, [settings]);

    useEffect(() => {
        checkActiveDownloads();
    }, []);

    const checkActiveDownloads = async () => {
        try {
            const downloads = await invoke<any[]>("get_downloads");
            const active = downloads.some(d => d.status === "downloading");
            setHasActiveDownloads(active);
        } catch (err) {
            console.error("Failed to check active downloads:", err);
        }
    };

    /**
     * Persists all current state values to the backend database.
     */
    const handleSave = async () => {
        setIsSaving(true);
        try {
            await saveAll(localSettings);
            setShowSuccess(true);
            setTimeout(() => setShowSuccess(false), 3000);
        } catch (err) {
            console.error("Failed to save settings:", err);
        } finally {
            setIsSaving(false);
        }
    };

    const handleChange = (key: keyof SettingsState, value: string | boolean) => {
        setLocalSettings(prev => ({ ...prev, [key]: value }));
    };

    const handleBrowse = async () => {
        try {
            const selected = await open({
                directory: true,
                multiple: false,
                defaultPath: settings.download_path,
            });

            if (selected) {
                // Open returns null if cancelled, string if single selection, string[] if multiple
                // Since multiple is false, it returns string | null
                const path = Array.isArray(selected) ? selected[0] : selected;
                if (path) {
                    handleChange("download_path", path);
                }
            }
        } catch (err) {
            console.error("Failed to open dialog:", err);
        }
    };

    /**
     * Reusable UI Components for Consistency
     */
    const SettingItem = ({ label, description, children }: { label: string, description: string, children: React.ReactNode }) => (
        <div className="flex items-center justify-between p-6 bg-brand-secondary border border-surface-border rounded-xl transition-all duration-200">
            <div>
                <h4 className="text-base font-medium text-text-primary mb-1">{label}</h4>
                <p className="text-xs text-text-tertiary">{description}</p>
            </div>
            {children}
        </div>
    );

    const SettingToggle = ({ enabled, onToggle }: { enabled: boolean, onToggle: () => void }) => (
        <button
            onClick={onToggle}
            className={clsx(
                "w-12 h-6 rounded-full transition-colors duration-500 ease-in-out relative shrink-0 border border-surface-border/50",
                enabled ? 'bg-text-primary' : 'bg-brand-tertiary'
            )}
        >
            <motion.div
                initial={false}
                animate={{
                    x: enabled ? 28 : 4,
                    backgroundColor: enabled ? "#09090b" : "#f4f4f5"
                }}
                transition={{
                    type: "spring",
                    stiffness: 500,
                    damping: 30,
                    mass: 0.8
                }}
                className="absolute top-1 w-4 h-4 rounded-full shadow-[0_1px_3px_rgba(0,0,0,0.2)]"
            />
        </button>
    );

    const sections = [
        { id: "general", title: "General", icon: Folder },
        { id: "network", title: "Network", icon: Globe },
        { id: "performance", title: "Performance", icon: Gauge },
        { id: "automation", title: "Automation", icon: Cpu },
        { id: "privacy", title: "Privacy", icon: Shield },
        { id: "about", title: "About", icon: Info },
    ];

    const renderSectionContent = () => {
        switch (activeSection) {
            case "general":
                return (
                    <div className="space-y-8 animate-fade-in">
                        <div className="space-y-4">
                            <label className="text-sm font-semibold text-text-secondary uppercase tracking-wider">Default Download Path</label>
                            <div className="flex gap-3">
                                <input
                                    type="text"
                                    value={settings.download_path}
                                    onChange={(e) => handleChange("download_path", e.target.value)}
                                    className="flex-1 bg-brand-primary border border-surface-border rounded-lg px-4 py-3 text-sm text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono"
                                />
                                <button
                                    onClick={handleBrowse}
                                    className="px-6 py-3 bg-brand-secondary border border-surface-border rounded-lg text-sm font-medium text-text-primary hover:bg-brand-tertiary transition-colors"
                                >
                                    Browse
                                </button>
                            </div>
                            <p className="text-xs text-text-tertiary font-medium">All your downloads will be saved here unless specified otherwise.</p>
                        </div>

                        <SettingItem
                            label="Ask for location"
                            description="Select download location manually for every new task."
                        >
                            <SettingToggle
                                enabled={settings.ask_location}
                                onToggle={() => handleChange("ask_location", !settings.ask_location)}
                            />
                        </SettingItem>
                    </div>
                );
            case "network":
                const displayLimit = (() => {
                    const b = parseInt(settings.speed_limit);
                    if (b === 0) return { val: "", unit: "MB/s" };
                    if (b < 1048576) return { val: (b / 1024).toFixed(0), unit: "KB/s" };
                    return { val: (b / 1048576).toFixed(0), unit: "MB/s" };
                })();

                const handleSpeedChange = (val: string, unit: string) => {
                    if (val === "" || parseInt(val) <= 0) {
                        handleChange("speed_limit", "0");
                        return;
                    }
                    const num = parseInt(val);
                    const multiplier = unit === "KB/s" ? 1024 : 1048576;
                    handleChange("speed_limit", (num * multiplier).toString());
                };

                return (
                    <div className="space-y-8 animate-fade-in">
                        <div className="space-y-4">
                            <div className="flex items-center gap-2">
                                <label className="text-sm font-semibold text-text-secondary uppercase tracking-wider">Global Speed Limit</label>
                                <div className="group relative">
                                    <Info size={14} className="text-text-tertiary cursor-help hover:text-text-primary transition-colors" />
                                    <div className="absolute top-full left-1/2 -translate-x-1/2 mt-2 w-64 p-3 bg-brand-tertiary border border-surface-border rounded-lg shadow-xl opacity-0 group-hover:opacity-100 pointer-events-none transition-opacity z-30 text-[10px] leading-relaxed text-text-secondary">
                                        For limits below <span className="text-text-primary font-bold">2MB/s</span>, Ciel automatically reduces connections to keep flows healthy and avoid Google Drive resets.
                                        <div className="absolute bottom-full left-1/2 -translate-x-1/2 border-8 border-transparent border-b-brand-tertiary"></div>
                                    </div>
                                </div>
                            </div>

                            <div className="flex items-center gap-3">
                                <div className="flex-1 flex gap-2">
                                    <input
                                        type="number"
                                        placeholder="0 (Unlimited)"
                                        value={displayLimit.val}
                                        onChange={(e) => handleSpeedChange(e.target.value, displayLimit.unit)}
                                        className="w-full bg-brand-primary border border-surface-border rounded-lg px-4 py-2.5 text-sm text-text-primary focus:outline-none focus:border-text-secondary transition-all font-mono"
                                    />
                                    <select
                                        value={displayLimit.unit}
                                        onChange={(e) => handleSpeedChange(displayLimit.val, e.target.value)}
                                        className="bg-brand-secondary border border-surface-border rounded-lg px-3 py-2 text-xs text-text-primary focus:outline-none focus:border-text-secondary transition-all cursor-pointer"
                                    >
                                        <option value="KB/s">KB/s</option>
                                        <option value="MB/s">MB/s</option>
                                    </select>
                                </div>
                                {settings.speed_limit !== "0" && (
                                    <button
                                        onClick={() => handleChange("speed_limit", "0")}
                                        className="px-4 py-2.5 bg-brand-tertiary hover:bg-brand-tertiary/80 text-text-tertiary hover:text-text-primary text-xs font-medium rounded-lg border border-surface-border transition-all"
                                    >
                                        Clear
                                    </button>
                                )}
                            </div>
                            <p className="text-xs text-text-tertiary font-medium">Limits total download bandwidth. Set to 0 for unlimited speed.</p>
                        </div>

                        <div className="text-center py-6 border-t border-surface-border mt-8">
                            <div className="w-12 h-12 rounded-full bg-brand-tertiary flex items-center justify-center mx-auto mb-3 text-text-tertiary opacity-50">
                                <Globe size={24} />
                            </div>
                            <h4 className="text-sm font-medium text-text-secondary mb-1">More Network Options</h4>
                            <p className="text-[10px] text-text-tertiary px-12">Proxies and Advanced User-Agents are under active development.</p>
                        </div>
                    </div>
                );
            case "performance":
                return (
                    <div className="space-y-8 animate-fade-in">
                        {hasActiveDownloads && (
                            <div className="flex items-start gap-3 p-4 rounded-lg bg-status-warning/10 border border-status-warning/20 text-status-warning">
                                <AlertTriangle size={18} className="mt-0.5" />
                                <div>
                                    <h4 className="text-sm font-medium mb-1">Active Downloads Detected</h4>
                                    <p className="text-xs opacity-90">
                                        Changes to connection limits will only apply to new downloads.
                                        <strong> Pause and resume</strong> active downloads to apply the new limit.
                                    </p>
                                </div>
                            </div>
                        )}

                        <div className="space-y-4">
                            <div className="flex items-center gap-2">
                                <label className="text-sm font-semibold text-text-secondary uppercase tracking-wider">Concurrent Connections</label>
                                <div className="group relative">
                                    <Info size={14} className="text-text-tertiary cursor-help hover:text-text-primary transition-colors" />
                                    <div className="absolute top-full left-1/2 -translate-x-1/2 mt-2 w-64 p-3 bg-brand-tertiary border border-surface-border rounded-lg shadow-xl opacity-0 group-hover:opacity-100 pointer-events-none transition-opacity z-30 text-[10px] leading-relaxed text-text-secondary">
                                        Google and many other servers might rate limit you. The <span className="text-text-primary font-bold">sweet/safe spot</span> for concurrent connections is <span className="text-text-primary font-bold">8-16</span>.
                                        <div className="absolute bottom-full left-1/2 -translate-x-1/2 border-8 border-transparent border-b-brand-tertiary"></div>
                                    </div>
                                </div>
                            </div>
                            <div className="flex items-center gap-4">
                                <input
                                    type="range"
                                    min="1"
                                    max="64"
                                    value={settings.max_connections}
                                    onChange={(e) => handleChange("max_connections", e.target.value)}
                                    className="flex-1 h-2 bg-brand-tertiary rounded-lg appearance-none cursor-pointer accent-text-primary"
                                />
                                <div className="w-16 h-10 flex items-center justify-center bg-brand-secondary rounded-lg border border-surface-border text-text-primary font-bold font-mono">
                                    {settings.max_connections}
                                </div>
                            </div>
                            <div className="space-y-2">
                                <p className="text-xs text-text-tertiary font-medium">Number of parallel streams used to pull a single file faster.</p>
                                {parseInt(settings.max_connections) > 32 && (
                                    <div className="flex items-start gap-2 p-3 rounded bg-status-error/10 border border-status-error/20 text-status-error text-[10px] leading-relaxed">
                                        <AlertTriangle size={14} className="mt-0.5 flex-shrink-0" />
                                        <p>
                                            <strong>CAUTION:</strong> Over 32 connections may lead to IP bans or rate-limiting.
                                        </p>
                                    </div>
                                )}
                            </div>
                        </div>

                        <div className="space-y-4">
                            <label className="text-sm font-semibold text-text-secondary uppercase tracking-wider">Simultaneous Downloads</label>
                            <div className="flex items-center gap-4">
                                <input
                                    type="range"
                                    min="1"
                                    max="10"
                                    value={settings.max_concurrent}
                                    onChange={(e) => handleChange("max_concurrent", e.target.value)}
                                    className="flex-1 h-2 bg-brand-tertiary rounded-lg appearance-none cursor-pointer accent-text-primary"
                                />
                                <div className="w-16 h-10 flex items-center justify-center bg-brand-secondary rounded-lg border border-surface-border text-text-primary font-bold font-mono">
                                    {settings.max_concurrent}
                                </div>
                            </div>
                            <p className="text-xs text-text-tertiary font-medium">How many different files Ciel will download at once.</p>
                        </div>

                        <SettingItem
                            label="Auto-Resume Downloads"
                            description="Automatically resume interrupted downloads when app starts."
                        >
                            <SettingToggle
                                enabled={settings.auto_resume}
                                onToggle={() => handleChange("auto_resume", !settings.auto_resume)}
                            />
                        </SettingItem>
                    </div>
                );
            case "automation":
                return (
                    <div className="space-y-8 animate-fade-in">
                        <SettingItem
                            label="Open folder on finish"
                            description="Automatically open the download folder when completed."
                        >
                            <SettingToggle
                                enabled={settings.open_folder_on_finish}
                                onToggle={() => handleChange("open_folder_on_finish", !settings.open_folder_on_finish)}
                            />
                        </SettingItem>

                        <SettingItem
                            label="Auto-Organize"
                            description="Automatically sort downloads into category-specific folders."
                        >
                            <SettingToggle
                                enabled={settings.auto_organize}
                                onToggle={() => handleChange("auto_organize", !settings.auto_organize)}
                            />
                        </SettingItem>

                        <SettingItem
                            label="Shutdown when done"
                            description="Shutdown the PC automatically after all downloads are finished."
                        >
                            <SettingToggle
                                enabled={settings.shutdown_on_finish}
                                onToggle={() => handleChange("shutdown_on_finish", !settings.shutdown_on_finish)}
                            />
                        </SettingItem>

                        <SettingItem
                            label="Sound Notifications"
                            description="Play a subtle sound when a download task completes."
                        >
                            <SettingToggle
                                enabled={localSettings.sound_on_finish}
                                onToggle={() => handleChange("sound_on_finish", !localSettings.sound_on_finish)}
                            />
                        </SettingItem>

                        <div className="pt-4 border-t border-brand-tertiary/20">
                            <h3 className="text-sm font-medium text-text-primary flex items-center gap-2 mb-4">
                                <Clock size={16} className="text-text-primary" />
                                Download Scheduler
                            </h3>

                            <div className="space-y-4">
                                <div className="flex items-center justify-between">
                                    <div className="flex flex-col gap-0.5">
                                        <span className="text-sm font-medium text-text-primary tracking-tight">Enable Scheduler</span>
                                        <span className="text-xs text-text-tertiary">Manage downloads based on time</span>
                                    </div>
                                    <SettingToggle
                                        enabled={localSettings.scheduler_enabled}
                                        onToggle={() => handleChange('scheduler_enabled', !localSettings.scheduler_enabled)}
                                    />
                                </div>
                            </div>
                        </div>
                    </div>
                );
            case "privacy":
                const browsers = [
                    { id: "none", name: "None (Clean)" },
                    { id: "chrome", name: "Google Chrome" },
                    { id: "firefox", name: "Mozilla Firefox" },
                    { id: "edge", name: "Microsoft Edge" },
                    { id: "brave", name: "Brave Browser" },
                    { id: "opera", name: "Opera" },
                    { id: "vivaldi", name: "Vivaldi" },
                    { id: "safari", name: "Safari" },
                ];

                return (
                    <div className="space-y-8 animate-fade-in">
                        <div className="space-y-4">
                            <label className="text-sm font-semibold text-text-secondary uppercase tracking-wider">Browser Authentication</label>
                            <div className="relative">
                                <select
                                    value={settings.cookie_browser}
                                    onChange={(e) => handleChange("cookie_browser", e.target.value)}
                                    className="w-full bg-brand-primary border border-surface-border rounded-lg px-4 py-3 text-sm text-text-primary appearance-none focus:outline-none focus:border-text-secondary transition-all cursor-pointer"
                                >
                                    {browsers.map(b => (
                                        <option key={b.id} value={b.id} className="bg-brand-secondary text-text-primary">
                                            {b.name}
                                        </option>
                                    ))}
                                </select>
                                <div className="absolute right-4 top-1/2 -translate-y-1/2 pointer-events-none text-text-tertiary">
                                    <Clock size={16} />
                                </div>
                            </div>
                            <p className="text-xs text-text-tertiary font-medium">
                                Extract session cookies to bypass 403 Forbidden errors.
                            </p>
                        </div>

                        <SettingItem
                            label="Autocatch (Clipboard)"
                            description="Automatically detect URLs in your clipboard."
                        >
                            <SettingToggle
                                enabled={settings.autocatch_enabled}
                                onToggle={() => handleChange("autocatch_enabled", !settings.autocatch_enabled)}
                            />
                        </SettingItem>

                        <SettingItem
                            label="Torrent Encryption"
                            description="Ciel currently uses the librqbit engine for Protocol Encryption(PE), for more privacy and security please use a VPN."
                        >
                            <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-text-primary/5 border border-text-primary/10 text-text-secondary text-[10px] font-medium">
                                <Shield size={10} className="text-text-primary" />
                                <span>AUTOMATED</span>
                            </div>
                        </SettingItem>
                    </div>
                );
            case "about":
                return (
                    <div className="space-y-6 text-center py-4 animate-fade-in">
                        <div className="relative w-24 h-24 mx-auto">
                            <div className="relative w-full h-full flex items-center justify-center p-2">
                                <img src={logo} alt="Ciel Logo" className="w-full h-full object-contain filter drop-shadow-[0_0_15px_rgba(255,255,255,0.1)]" />
                            </div>
                        </div>

                        <div>
                            <h2 className="text-2xl font-bold text-text-primary mb-1 tracking-tight">Ciel Download Manager</h2>
                            <div className="inline-flex items-center gap-2 px-3 py-1 rounded-full bg-brand-tertiary text-text-secondary text-[10px] font-mono font-medium lowercase">
                                v0.1.0-alpha
                            </div>
                        </div>

                        <p className="text-sm text-text-secondary leading-relaxed max-w-md mx-auto">Built for speed!</p>

                        <div className="flex justify-center my-4">
                            <button
                                onClick={() => checkForUpdates()}
                                disabled={checking}
                                className="px-5 py-2 bg-brand-secondary/50 hover:bg-brand-secondary border border-surface-border rounded-lg text-sm font-medium text-text-primary transition-all disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
                            >
                                {checking ? (
                                    <>
                                        <div className="w-3 h-3 border-2 border-text-tertiary border-t-text-primary rounded-full animate-spin" />
                                        <span>Checking...</span>
                                    </>
                                ) : (
                                    <>
                                        <Globe size={14} className="text-text-tertiary" />
                                        <span>Check for Updates</span>
                                    </>
                                )}
                            </button>
                        </div>

                        <div className="grid grid-cols-2 gap-4 max-w-sm mx-auto">
                            <a href="https://github.com/shubham-pathak1/ciel" target="_blank" rel="noopener noreferrer" className="flex items-center justify-center gap-2 px-4 py-2 rounded-lg bg-brand-secondary hover:bg-brand-tertiary text-text-secondary hover:text-text-primary transition-all border border-surface-border text-xs font-medium group">
                                <Github size={14} className="text-text-tertiary group-hover:text-text-primary transition-colors" />
                                GitHub
                            </a>
                            <a href="https://github.com/shubham-pathak1/ciel/blob/main/LICENSE" target="_blank" rel="noopener noreferrer" className="flex items-center justify-center gap-2 px-4 py-2 rounded-lg bg-brand-secondary hover:bg-brand-tertiary text-text-secondary hover:text-text-primary transition-all border border-surface-border text-xs font-medium group">
                                <FileText size={14} className="text-text-tertiary group-hover:text-text-primary transition-colors" />
                                License
                            </a>
                        </div>
                    </div>
                );
            default:
                return (
                    <div className="flex flex-col items-center justify-center h-64 text-center animate-fade-in">
                        <div className="w-16 h-16 rounded-full bg-brand-secondary flex items-center justify-center mb-4 text-text-tertiary">
                            <Info size={32} />
                        </div>
                        <h3 className="text-lg font-medium text-text-primary mb-2">Coming Soon</h3>
                        <p className="text-sm text-text-tertiary">This section is under development.</p>
                    </div>
                );
        }
    };

    return (
        <div className="h-full flex flex-col w-full max-w-5xl mx-auto">
            {/* Header */}
            <div className="mb-8 flex items-end justify-between sticky top-0 bg-brand-primary z-20 py-4 border-b border-transparent">
                <div>
                    <h1 className="text-2xl font-semibold text-text-primary tracking-tight mb-1">Settings</h1>
                    <p className="text-sm text-text-secondary">Personalize your downloading experience</p>
                </div>

                <button
                    onClick={handleSave}
                    disabled={isSaving}
                    className="btn-primary flex items-center gap-2 px-6 py-2.5"
                >
                    {isSaving ? (
                        <div className="w-4 h-4 border-2 border-brand-secondary border-t-text-primary rounded-full animate-spin" />
                    ) : showSuccess ? (
                        <Check className="w-4 h-4" />
                    ) : (
                        <Save className="w-4 h-4" />
                    )}
                    <span className="font-semibold">{showSuccess ? "Saved" : "Save Changes"}</span>
                </button>
            </div>

            <div className="flex-1 flex gap-8 min-h-0 overflow-hidden pb-6">
                {/* Sidemenu */}
                <div className="w-56 space-y-1 overflow-y-auto pr-2">
                    {sections.map((section) => {
                        const Icon = section.icon;
                        const isActive = activeSection === section.id;
                        return (
                            <button
                                key={section.id}
                                onClick={() => setActiveSection(section.id)}
                                className={clsx(
                                    "w-full flex items-center gap-3 px-4 py-3 rounded-lg text-sm font-medium transition-all duration-200",
                                    isActive
                                        ? "bg-brand-tertiary text-text-primary"
                                        : "text-text-secondary hover:bg-brand-tertiary/50 hover:text-text-primary"
                                )}
                            >
                                <Icon size={18} className={isActive ? "text-text-primary" : "text-text-tertiary group-hover:text-text-primary"} />
                                <span>{section.title}</span>
                            </button>
                        );
                    })}
                </div>

                {/* Content */}
                <div className="flex-1 card-base p-8 overflow-y-auto scrollbar-hide">
                    {renderSectionContent()}
                </div>
            </div>
        </div>
    );
}
