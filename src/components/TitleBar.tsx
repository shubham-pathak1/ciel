/**
 * @file TitleBar.tsx
 * @description Custom frameless window title bar.
 * Implements window decoration and management (minimize, maximize, close) for the Tauri app.
 */

import { Minus, Square, X } from "lucide-react";
import logo from "../assets/logo.png";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * TitleBar Component.
 * 
 * Responsibilities:
 * - Provides a "drag region" for moving the frameless window.
 * - Handles native window lifecycle events (minimize, maximize, close) via IPC.
 * - Displays application branding.
 */
export function TitleBar() {
    const appWindow = getCurrentWindow();

    const handleMinimize = async () => {
        console.log("Minimize clicked");
        try {
            await appWindow.minimize();
        } catch (e) {
            console.error("Minimize error:", e);
        }
    };

    const handleMaximize = async () => {
        console.log("Maximize clicked");
        try {
            await appWindow.toggleMaximize();
        } catch (e) {
            console.error("Maximize error:", e);
        }
    };

    const handleClose = async () => {
        console.log("Close clicked");
        try {
            await appWindow.close();
        } catch (e) {
            console.error("Close error:", e);
        }
    };

    return (
        <div className="h-10 w-full flex items-center justify-between px-4 drag-region select-none z-50 bg-brand-primary border-b border-transparent">
            {/* Logo area */}
            <div className="flex items-center gap-3 opacity-90 hover:opacity-100 transition-opacity duration-300">
                <img
                    src={logo}
                    alt="Ciel Logo"
                    className="h-6 w-auto object-contain filter drop-shadow-[0_0_8px_rgba(255,255,255,0.15)]"
                />
                <span className="text-sm font-bold text-text-primary tracking-tight">
                    Ciel
                </span>
            </div>

            {/* Window Controls - Custom Modern Style */}
            <div className="flex items-center gap-2 no-drag">
                <button
                    onClick={handleMinimize}
                    className="w-8 h-7 flex items-center justify-center rounded hover:bg-brand-tertiary text-text-secondary hover:text-text-primary transition-all duration-200"
                    aria-label="Minimize"
                >
                    <Minus size={14} />
                </button>
                <button
                    onClick={handleMaximize}
                    className="w-8 h-7 flex items-center justify-center rounded hover:bg-brand-tertiary text-text-secondary hover:text-text-primary transition-all duration-200"
                    aria-label="Maximize"
                >
                    <Square size={12} />
                </button>
                <button
                    onClick={handleClose}
                    className="w-8 h-7 flex items-center justify-center rounded hover:bg-red-500/10 hover:text-red-500 text-text-secondary transition-all duration-200"
                    aria-label="Close"
                >
                    <X size={14} />
                </button>
            </div>
        </div>
    );
}
