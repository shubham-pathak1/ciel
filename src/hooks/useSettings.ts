import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface SettingsState {
    download_path: string;
    max_connections: string;
    max_concurrent: string;
    auto_resume: boolean;
    theme: string;
    ask_location: boolean;
    autocatch_enabled: boolean;
    speed_limit: string;
    torrent_encryption: boolean;
    open_folder_on_finish: boolean;
    shutdown_on_finish: boolean;
    sound_on_finish: boolean;
    scheduler_enabled: boolean;
    scheduler_start_time: string;
    scheduler_pause_time: string;
    auto_organize: boolean;
    cookie_browser: string;
}

const DEFAULT_SETTINGS: SettingsState = {
    download_path: "./downloads",
    max_connections: "8",
    max_concurrent: "3",
    auto_resume: true,
    theme: "dark",
    ask_location: false,
    autocatch_enabled: true,
    speed_limit: "0",
    torrent_encryption: false,
    open_folder_on_finish: false,
    shutdown_on_finish: false,
    sound_on_finish: true,
    scheduler_enabled: false,
    scheduler_start_time: "02:00",
    scheduler_pause_time: "08:00",
    auto_organize: false,
    cookie_browser: "none",
};

// Simple global observers to sync multiple hook instances
const observers: Array<(s: SettingsState) => void> = [];

export function useSettings() {
    const [settings, setSettings] = useState<SettingsState>(DEFAULT_SETTINGS);
    const [isLoading, setIsLoading] = useState(true);

    const loadSettings = useCallback(async () => {
        try {
            const result = await invoke<Record<string, string>>("get_settings");
            const newSettings: SettingsState = {
                download_path: result.download_path || DEFAULT_SETTINGS.download_path,
                max_connections: result.max_connections || DEFAULT_SETTINGS.max_connections,
                max_concurrent: result.max_concurrent || DEFAULT_SETTINGS.max_concurrent,
                auto_resume: result.auto_resume === "true",
                theme: result.theme || DEFAULT_SETTINGS.theme,
                ask_location: result.ask_location === "true",
                autocatch_enabled: result.autocatch_enabled === "true",
                speed_limit: result.speed_limit || DEFAULT_SETTINGS.speed_limit,
                torrent_encryption: result.torrent_encryption === "true",
                open_folder_on_finish: result.open_folder_on_finish === "true",
                shutdown_on_finish: result.shutdown_on_finish === "true",
                sound_on_finish: result.sound_on_finish === "true",
                scheduler_enabled: result.scheduler_enabled === "true",
                scheduler_start_time: result.scheduler_start_time || DEFAULT_SETTINGS.scheduler_start_time,
                scheduler_pause_time: result.scheduler_pause_time || DEFAULT_SETTINGS.scheduler_pause_time,
                auto_organize: result.auto_organize === "true",
                cookie_browser: result.cookie_browser || DEFAULT_SETTINGS.cookie_browser,
            };
            setSettings(newSettings);
            observers.forEach(obs => obs(newSettings));
        } catch (err) {
            console.error("Failed to load settings:", err);
        } finally {
            setIsLoading(false);
        }
    }, []);

    useEffect(() => {
        loadSettings();

        const observer = (newSettings: SettingsState) => setSettings(newSettings);
        observers.push(observer);

        return () => {
            const index = observers.indexOf(observer);
            if (index > -1) observers.splice(index, 1);
        };
    }, [loadSettings]);

    const updateSetting = async (key: keyof SettingsState, value: string | boolean) => {
        const stringValue = typeof value === 'boolean' ? String(value) : value;
        try {
            // Optimistic update
            const nextSettings = { ...settings, [key]: value };
            setSettings(nextSettings);
            observers.forEach(obs => obs(nextSettings));

            await invoke("update_setting", { key, value: stringValue });
        } catch (err) {
            console.error(`Failed to update setting ${key}:`, err);
            // Revert on error
            loadSettings();
        }
    };

    const saveAll = async (newSettings: SettingsState) => {
        try {
            setSettings(newSettings);
            observers.forEach(obs => obs(newSettings));
            for (const [key, value] of Object.entries(newSettings)) {
                const stringValue = typeof value === 'boolean' ? String(value) : value;
                await invoke("update_setting", { key, value: stringValue });
            }
        } catch (err) {
            console.error("Failed to save all settings:", err);
            loadSettings();
        }
    };

    return { settings, updateSetting, saveAll, isLoading, refresh: loadSettings };
}
