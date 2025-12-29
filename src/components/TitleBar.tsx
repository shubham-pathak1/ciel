import { Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

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
            <div className="flex items-center gap-2.5 opacity-90 hover:opacity-100 transition-opacity duration-300">
                <div className="w-6 h-6 rounded-md bg-brand-secondary border border-surface-border flex items-center justify-center">
                    <span className="text-text-primary font-bold text-xs">C</span>
                </div>
                <span className="text-sm font-semibold text-text-primary tracking-tight">
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
