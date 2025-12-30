import { useState } from "react";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { DownloadQueue } from "./components/DownloadQueue";
import { Settings } from "./components/Settings";
import { History } from "./components/History";

type View = "downloads" | "active" | "completed" | "settings";

function App() {
    const [currentView, setCurrentView] = useState<View>("downloads");

    const renderContent = () => {
        switch (currentView) {
            case "settings":
                return <Settings />;
            case "completed":
                return <History />;
            case "downloads":
            case "active":
            default:
                return <DownloadQueue filter={currentView} />;
        }
    };

    return (
        <div className="h-screen w-screen flex flex-col bg-brand-primary text-text-primary overflow-hidden font-sans relative selection:bg-zinc-700 selection:text-white">
            <TitleBar />

            <div className="flex flex-1 overflow-hidden">
                <Sidebar currentView={currentView} onViewChange={setCurrentView} />

                <main className="flex-1 bg-brand-primary flex flex-col min-w-0">
                    <div className="flex-1 overflow-auto p-8 scrollbar-hide">
                        {renderContent()}
                    </div>
                </main>
            </div>
        </div>
    );
}

export default App;
