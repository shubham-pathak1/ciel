import {
    Download,
    PlayCircle,
    CheckCircle,
    Settings as SettingsIcon,
} from "lucide-react";
import clsx from "clsx";

type View = "downloads" | "active" | "completed" | "settings";

interface SidebarProps {
    currentView: View;
    onViewChange: (view: View) => void;
}

interface NavItem {
    id: View;
    label: string;
    icon: typeof Download;
}

const navItems: NavItem[] = [
    { id: "downloads", label: "All Downloads", icon: Download },
    { id: "active", label: "Active", icon: PlayCircle },
    { id: "completed", label: "Finished", icon: CheckCircle },
    { id: "settings", label: "Settings", icon: SettingsIcon },
];

export function Sidebar({ currentView, onViewChange }: SidebarProps) {
    return (
        <aside className="w-64 flex flex-col items-center py-6 px-4 bg-brand-primary border-r border-surface-border z-20">
            {/* Navigation */}
            <nav className="w-full flex-1">
                <ul className="space-y-1">
                    {navItems.map((item) => {
                        const Icon = item.icon;
                        const isActive = currentView === item.id;

                        return (
                            <li key={item.id}>
                                <button
                                    onClick={() => onViewChange(item.id)}
                                    className={clsx(
                                        "w-full flex items-center gap-3 px-3 py-2 rounded-lg transition-all duration-200 text-sm",
                                        isActive
                                            ? "bg-brand-tertiary text-text-primary font-medium"
                                            : "text-text-secondary hover:bg-brand-tertiary/50 hover:text-text-primary"
                                    )}
                                >
                                    <Icon size={18} className={isActive ? "text-text-primary" : "text-text-tertiary group-hover:text-text-primary"} />
                                    <span>{item.label}</span>
                                </button>
                            </li>
                        );
                    })}
                </ul>
            </nav>

        </aside>
    );
}
