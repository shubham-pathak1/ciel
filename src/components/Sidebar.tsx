/**
 * @file Sidebar.tsx
 * @description Main navigation component for the application.
 * Manages view switching and category-based filtering for the download queue.
 */

import {
    Download,
    PlayCircle,
    CheckCircle,
    Settings as SettingsIcon,
    Video,
    Music,
    Archive,
    Gamepad2,
    FileText,
    MoreHorizontal,
    Clock
} from "lucide-react";
import clsx from "clsx";
import { motion } from "framer-motion";
import { useSettings } from "../hooks/useSettings";

type View = "downloads" | "active" | "completed" | "settings" | "scheduler" | "Video" | "Audio" | "Compressed" | "Software" | "Documents" | "Other";

/**
 * Props for the Sidebar component.
 * @property currentView - The currently active view or category filter.
 * @property onViewChange - Callback to switch the active view.
 */
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

const categoryItems: NavItem[] = [
    { id: "Video", label: "Videos", icon: Video },
    { id: "Audio", label: "Music", icon: Music },
    { id: "Compressed", label: "Archives", icon: Archive },
    { id: "Software", label: "Software", icon: Gamepad2 },
    { id: "Documents", label: "Documents", icon: FileText },
    { id: "Other", label: "Other", icon: MoreHorizontal },
];

/**
 * Sidebar Component.
 * 
 * Responsibilities:
 * - Displays primary navigation items (All Downloads, Active, Finished, Settings).
 * - Displays a list of file categories for targeted filtering.
 * - Managed fluid transitions between views using `framer-motion`.
 */
export function Sidebar({ currentView, onViewChange }: SidebarProps) {
    const { settings } = useSettings();

    const mainNavItems = [...navItems];
    if (settings.scheduler_enabled) {
        // Insert before settings
        mainNavItems.splice(3, 0, { id: "scheduler", label: "Scheduler", icon: Clock });
    }

    const renderNavItem = (item: NavItem) => {
        const Icon = item.icon;
        const isActive = currentView === item.id;

        return (
            <li key={item.id} className="relative group flex justify-center">
                <button
                    onClick={() => onViewChange(item.id)}
                    aria-label={item.label}
                    aria-current={isActive ? "page" : undefined}
                    className={clsx(
                        "relative flex h-10 w-10 items-center justify-center rounded-xl transition-all duration-200",
                        isActive
                            ? "text-text-primary"
                            : "text-text-secondary hover:text-text-primary hover:bg-brand-tertiary/40"
                    )}
                >
                    {isActive && (
                        <motion.div
                            layoutId="active-pill"
                            className="absolute inset-0 bg-brand-tertiary rounded-xl"
                            transition={{ type: "spring", stiffness: 500, damping: 40 }}
                        />
                    )}
                    <Icon size={19} className={clsx("relative z-10", isActive ? "text-text-primary" : "text-text-tertiary")} />
                </button>

                <div className="pointer-events-none absolute left-[calc(100%+8px)] top-1/2 z-50 -translate-y-1/2 translate-x-0.5 rounded-lg border border-surface-border bg-black/90 px-2.5 py-1.5 text-[11px] font-semibold tracking-wide text-text-primary opacity-0 shadow-2xl backdrop-blur-md transition-all duration-150 group-hover:translate-x-0 group-hover:opacity-100 whitespace-nowrap">
                    <div className="absolute left-[-5px] top-1/2 h-2.5 w-2.5 -translate-y-1/2 rotate-45 border-b border-l border-surface-border bg-black/90" />
                    {item.label}
                </div>
            </li>
        );
    };

    return (
        <aside className="w-[72px] flex flex-col items-center py-6 px-1.5 bg-brand-primary border-r border-surface-border z-20">
            {/* Navigation */}
            <nav className="w-full flex-1">
                <ul className="space-y-2">
                    {mainNavItems.map(renderNavItem)}
                </ul>

                <div className="mx-auto my-5 h-px w-8 bg-surface-border" />
                <ul className="space-y-2">
                    {categoryItems.map(renderNavItem)}
                </ul>
            </nav>

        </aside>
    );
}
