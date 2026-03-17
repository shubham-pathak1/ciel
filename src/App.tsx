import { useState, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { DownloadQueue } from "./components/DownloadQueue";
import { Settings } from "./components/Settings";
import { History } from "./components/History";
import { Scheduler } from "./components/Scheduler";
import { useSettings } from "./hooks/useSettings";
import { motion, AnimatePresence } from "framer-motion";

type View = "downloads" | "active" | "completed" | "settings" | "scheduler" | "Video" | "Audio" | "Compressed" | "Software" | "Documents" | "Other";


function App() {
    const [currentView, setCurrentView] = useState<View>("downloads");
    const [isFullscreen, setIsFullscreen] = useState(false);
    const { settings } = useSettings();

    useEffect(() => {
        const appWindow = getCurrentWindow();
        let unlisten: (() => void) | null = null;

        const syncFullscreen = async () => {
            try {
                const fullscreen = await appWindow.isFullscreen();
                setIsFullscreen(fullscreen);
            } catch (err) {
                console.error("Fullscreen sync failed:", err);
            }
        };

        const handleKeyDown = async (event: KeyboardEvent) => {
            if (event.key !== "F11") return;
            event.preventDefault();
            try {
                const isFullscreen = await appWindow.isFullscreen();
                await appWindow.setFullscreen(!isFullscreen);
                setIsFullscreen(!isFullscreen);
            } catch (err) {
                console.error("Fullscreen toggle failed:", err);
            }
        };

        syncFullscreen();
        appWindow.onResized(() => {
            void syncFullscreen();
        }).then((u) => {
            unlisten = u;
        }).catch((err) => {
            console.error("Fullscreen listener failed:", err);
        });

        window.addEventListener("keydown", handleKeyDown);
        return () => {
            window.removeEventListener("keydown", handleKeyDown);
            if (unlisten) {
                unlisten();
            }
        };
    }, []);

    useEffect(() => {
        if (currentView === "scheduler" && !settings.scheduler_enabled) {
            setCurrentView("downloads");
        }
    }, [settings.scheduler_enabled, currentView]);

    const renderContent = () => {
        if (["Video", "Audio", "Compressed", "Software", "Documents", "Other"].includes(currentView)) {
            return <DownloadQueue filter="downloads" category={currentView} />;
        }

        switch (currentView) {
            case "settings":
                return <Settings />;
            case "completed":
                return <History />;
            case "scheduler":
                return <Scheduler />;
            case "active":
                return <DownloadQueue filter="active" />;
            case "downloads":
            default:
                return <DownloadQueue filter="downloads" />;
        }
    };

    return (
        <div className="h-screen w-screen flex flex-col bg-brand-primary text-text-primary overflow-hidden font-sans relative selection:bg-zinc-700 selection:text-white">
            <TitleBar isFullscreen={isFullscreen} />

            <div className="flex flex-1 overflow-hidden">
                <Sidebar currentView={currentView} onViewChange={setCurrentView} />

                <main className="flex-1 bg-brand-primary flex flex-col min-w-0">
                    <div className="flex-1 overflow-auto p-8 scrollbar-hide">
                        <AnimatePresence mode="popLayout">
                            <motion.div
                                key={currentView}
                                initial={{ opacity: 0, x: 4 }}
                                animate={{ opacity: 1, x: 0 }}
                                exit={{ opacity: 0, x: -4 }}
                                transition={{ duration: 0.15, ease: "easeOut" }}
                                className="h-full"
                            >
                                {renderContent()}
                            </motion.div>
                        </AnimatePresence>
                    </div>
                </main>
            </div>
        </div>
    );
}

export default App;
