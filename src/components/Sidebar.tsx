import {
    Download,
    PlayCircle,
    CheckCircle,
    Settings as SettingsIcon,
} from "lucide-react";
import clsx from "clsx";
import { motion } from "framer-motion";

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
                            <li key={item.id} className="relative">
                                <button
                                    onClick={() => onViewChange(item.id)}
                                    className={clsx(
                                        "w-full flex items-center gap-3 px-3 py-2 rounded-lg transition-all duration-200 text-sm relative z-10",
                                        isActive
                                            ? "text-text-primary font-medium"
                                            : "text-text-secondary hover:text-text-primary"
                                    )}
                                >
                                    {isActive && (
                                        <motion.div
                                            layoutId="active-pill"
                                            className="absolute inset-0 bg-brand-tertiary rounded-lg z-[-1]"
                                            transition={{ type: "spring", stiffness: 500, damping: 40 }}
                                        />
                                    )}
                                    <Icon size={18} className={isActive ? "text-text-primary" : "text-text-tertiary"} />
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
