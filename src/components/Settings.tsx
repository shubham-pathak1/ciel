import { useEffect, useState } from "react";
import logo from "../assets/logo.png";
import { Folder, Globe, Gauge, Shield, Info, Check, Save, AlertTriangle, Github, FileText } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import clsx from "clsx";
import { open } from "@tauri-apps/plugin-dialog";

interface SettingsState {
    download_path: string;
    max_connections: string;
    auto_resume: string;
    theme: string;
    ask_location: string;
    autocatch_enabled: string;
}

export function Settings() {
    const [settings, setSettings] = useState<SettingsState>({
        download_path: "./downloads",
        max_connections: "8",
        auto_resume: "true",
        theme: "dark",
        ask_location: "false",
        autocatch_enabled: "true",
    });
    const [activeSection, setActiveSection] = useState("general");
    const [isSaving, setIsSaving] = useState(false);
    const [showSuccess, setShowSuccess] = useState(false);
    const [hasActiveDownloads, setHasActiveDownloads] = useState(false);

    useEffect(() => {
        loadSettings();
    }, []);

    const loadSettings = async () => {
        try {
            const result = await invoke<Record<string, string>>("get_settings");
            setSettings({
                download_path: result.download_path || "./downloads",
                max_connections: result.max_connections || "8",
                auto_resume: result.auto_resume || "true",
                theme: result.theme || "dark",
                ask_location: result.ask_location || "false",
                autocatch_enabled: result.autocatch_enabled || "true",
            });

            // Check for active downloads
            const downloads = await invoke<any[]>("get_downloads");
            const active = downloads.some(d => d.status === "downloading");
            setHasActiveDownloads(active);
        } catch (err) {
            console.error("Failed to load settings:", err);
        }
    };

    const handleSave = async () => {
        setIsSaving(true);
        try {
            for (const [key, value] of Object.entries(settings)) {
                await invoke("update_setting", { key, value });
            }
            setShowSuccess(true);
            setTimeout(() => setShowSuccess(false), 3000);
        } catch (err) {
            console.error("Failed to save settings:", err);
        } finally {
            setIsSaving(false);
        }
    };

    const handleChange = (key: keyof SettingsState, value: string) => {
        setSettings(prev => ({ ...prev, [key]: value }));
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

    const sections = [
        { id: "general", title: "General", icon: Folder },
        { id: "network", title: "Network", icon: Globe },
        { id: "performance", title: "Performance", icon: Gauge },
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

                        <div className="flex items-center justify-between p-6 bg-brand-secondary border border-surface-border rounded-xl">
                            <div>
                                <h4 className="text-base font-medium text-text-primary mb-1">Ask for location</h4>
                                <p className="text-xs text-text-tertiary">Select download location manually for every new task.</p>
                            </div>
                            <button
                                onClick={() => handleChange("ask_location", settings.ask_location === "true" ? "false" : "true")}
                                className={clsx(
                                    "w-12 h-6 rounded-full transition-all duration-300 relative",
                                    settings.ask_location === "true" ? 'bg-text-primary' : 'bg-brand-tertiary'
                                )}
                            >
                                <div className={clsx(
                                    "absolute top-1 w-4 h-4 bg-brand-secondary rounded-full transition-transform duration-300",
                                    settings.ask_location === "true" ? 'translate-x-7' : 'translate-x-1'
                                )} />
                            </button>
                        </div>
                    </div>
                );
            case "network":
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
                            <label className="text-sm font-semibold text-text-secondary uppercase tracking-wider">Concurrent Connections</label>
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
                                <p className="text-xs text-text-tertiary font-medium">Higher values may increase speed but also server load.</p>
                                {parseInt(settings.max_connections) > 32 && (
                                    <div className="flex items-start gap-2 p-3 rounded bg-status-error/10 border border-status-error/20 text-status-error text-[10px] leading-relaxed">
                                        <AlertTriangle size={14} className="mt-0.5 flex-shrink-0" />
                                        <p>
                                            <strong>CAUTION:</strong> Using more than 32 connections may lead to temporary IP bans or rate-limiting by platforms like YouTube or Google Drive. Use at your own risk.
                                        </p>
                                    </div>
                                )}
                            </div>
                        </div>

                        <div className="flex items-center justify-between p-6 bg-brand-secondary border border-surface-border rounded-xl">
                            <div>
                                <h4 className="text-base font-medium text-text-primary mb-1">Auto-Resume Downloads</h4>
                                <p className="text-xs text-text-tertiary">Automatically resume interrupted downloads when app starts.</p>
                            </div>
                            <button
                                onClick={() => handleChange("auto_resume", settings.auto_resume === "true" ? "false" : "true")}
                                className={clsx(
                                    "w-12 h-6 rounded-full transition-all duration-300 relative",
                                    settings.auto_resume === "true" ? 'bg-text-primary' : 'bg-brand-tertiary'
                                )}
                            >
                                <div className={clsx(
                                    "absolute top-1 w-4 h-4 bg-brand-secondary rounded-full transition-transform duration-300",
                                    settings.auto_resume === "true" ? 'translate-x-7' : 'translate-x-1'
                                )} />
                            </button>
                        </div>
                    </div>
                );
            case "privacy":
                return (
                    <div className="space-y-8 animate-fade-in">
                        <div className="flex items-center justify-between p-6 bg-brand-secondary border border-surface-border rounded-xl">
                            <div>
                                <h4 className="text-base font-medium text-text-primary mb-1">Autocatch (Clipboard)</h4>
                                <p className="text-xs text-text-tertiary">Automatically detect and prompt for URLs in your clipboard.</p>
                            </div>
                            <button
                                onClick={() => handleChange("autocatch_enabled", settings.autocatch_enabled === "true" ? "false" : "true")}
                                className={clsx(
                                    "w-12 h-6 rounded-full transition-all duration-300 relative",
                                    settings.autocatch_enabled === "true" ? 'bg-text-primary' : 'bg-brand-tertiary'
                                )}
                            >
                                <div className={clsx(
                                    "absolute top-1 w-4 h-4 bg-brand-secondary rounded-full transition-transform duration-300",
                                    settings.autocatch_enabled === "true" ? 'translate-x-7' : 'translate-x-1'
                                )} />
                            </button>
                        </div>
                    </div>
                );
            case "about":
                return (
                    <div className="space-y-8 text-center py-8 animate-fade-in">
                        <div className="relative w-32 h-32 mx-auto">
                            <div className="relative w-full h-full flex items-center justify-center p-4">
                                <img
                                    src={logo}
                                    alt="Ciel Logo"
                                    className="w-full h-full object-contain filter drop-shadow-[0_0_15px_rgba(255,255,255,0.1)]"
                                />
                            </div>
                        </div>

                        <div>
                            <h2 className="text-2xl font-bold text-text-primary mb-2 tracking-tight">Ciel Download Manager</h2>
                            <div className="inline-flex items-center gap-2 px-3 py-1 rounded-full bg-brand-tertiary text-text-secondary text-xs font-mono font-medium">
                                v0.1.0 Beta
                            </div>
                        </div>

                        <div className="max-w-md mx-auto p-6 bg-brand-secondary rounded-xl border border-surface-border">
                            <p className="text-sm text-text-secondary italic leading-relaxed">
                                "Built for speed, designed for elegance. Ciel redefines what a download manager can be."
                            </p>
                        </div>

                        <div className="grid grid-cols-2 gap-4 max-w-sm mx-auto">
                            <a
                                href="https://github.com/shubham-pathak1/ciel"
                                target="_blank"
                                rel="noopener noreferrer"
                                className="flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg bg-brand-secondary hover:bg-brand-tertiary text-text-secondary hover:text-text-primary transition-all border border-surface-border text-xs font-medium group"
                            >
                                <Github size={16} className="text-text-tertiary group-hover:text-text-primary transition-colors" />
                                GitHub
                            </a>
                            <a
                                href="https://github.com/shubham-pathak1/ciel/blob/main/LICENSE"
                                target="_blank"
                                rel="noopener noreferrer"
                                className="flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg bg-brand-secondary hover:bg-brand-tertiary text-text-secondary hover:text-text-primary transition-all border border-surface-border text-xs font-medium group"
                            >
                                <FileText size={16} className="text-text-tertiary group-hover:text-text-primary transition-colors" />
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
                        <p className="text-sm text-text-tertiary">This setting section is under development.</p>
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
