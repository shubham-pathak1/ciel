import { motion } from 'framer-motion';
import { Trash2, Check } from 'lucide-react';
import { ModalPortal } from './ModalPortal';

interface ConfirmDialogProps {
    isOpen: boolean;
    onClose: () => void;
    onConfirm: () => void;
    title: string;
    message: string;
    confirmText?: string;
    cancelText?: string;
    showCheckbox?: boolean;
    checkboxChecked?: boolean;
    onCheckboxChange?: (checked: boolean) => void;
    isLoading?: boolean;
}

export function ConfirmDialog({
    isOpen,
    onClose,
    onConfirm,
    title,
    message,
    confirmText = "Confirm",
    cancelText = "Cancel",
    showCheckbox = false,
    checkboxChecked = false,
    onCheckboxChange,
    isLoading = false
}: ConfirmDialogProps) {
    if (!isOpen) return null;

    return (
        <ModalPortal>
            <div className="fixed inset-0 z-[100] flex items-center justify-center p-4">
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    exit={{ opacity: 0 }}
                    onClick={onClose}
                    className="absolute inset-0 bg-black/60 backdrop-blur-sm"
                />
                <motion.div
                    initial={{ opacity: 0, scale: 0.95, y: 20 }}
                    animate={{ opacity: 1, scale: 1, y: 0 }}
                    exit={{ opacity: 0, scale: 0.95, y: 20 }}
                    className="relative w-full max-w-md bg-brand-primary border border-brand-tertiary/30 rounded-2xl shadow-2xl overflow-hidden"
                >
                    <div className="p-6">
                        <div className="flex items-center gap-3 mb-4">
                            <div className="w-10 h-10 rounded-xl bg-status-error/10 flex items-center justify-center text-status-error">
                                <Trash2 size={20} />
                            </div>
                            <h2 className="text-lg font-bold text-text-primary">{title}</h2>
                        </div>
                        <p className="text-sm text-text-secondary mb-6 leading-relaxed">
                            {message}
                        </p>

                        {showCheckbox && (
                            <label className="flex items-center gap-3 p-4 rounded-xl bg-white/5 border border-brand-tertiary/20 hover:border-brand-tertiary/40 cursor-pointer transition-all mb-6 group">
                                <div className="relative flex items-center justify-center">
                                    <input
                                        type="checkbox"
                                        checked={checkboxChecked}
                                        onChange={(e) => onCheckboxChange?.(e.target.checked)}
                                        className="peer appearance-none w-4 h-4 rounded border border-white/60 checked:bg-white checked:border-white transition-all cursor-pointer"
                                    />
                                    <Check size={14} className="absolute text-black opacity-0 peer-checked:opacity-100 transition-opacity pointer-events-none" />
                                </div>
                                <span className="text-sm font-medium text-text-primary group-hover:text-white transition-colors">
                                    Delete selected files from the disk
                                </span>
                            </label>
                        )}

                        <div className="flex gap-3">
                            <button
                                onClick={onClose}
                                className="flex-1 px-4 py-2.5 rounded-xl text-sm font-bold text-black bg-white border border-brand-tertiary/20 hover:bg-white/90 transition-all active:scale-95 shadow-sm"
                            >
                                {cancelText}
                            </button>
                            <button
                                onClick={onConfirm}
                                disabled={isLoading}
                                className="flex-1 px-4 py-2.5 rounded-xl text-sm font-bold text-black bg-white hover:bg-white/90 transition-all active:scale-95 shadow-sm disabled:opacity-50 disabled:cursor-not-allowed flex items-center justify-center gap-2"
                            >
                                {isLoading && <div className="w-3.5 h-3.5 border-2 border-black border-t-transparent rounded-full animate-spin" />}
                                {isLoading ? "Deleting..." : confirmText}
                            </button>
                        </div>
                    </div>
                </motion.div>
            </div>
        </ModalPortal>
    );
}
