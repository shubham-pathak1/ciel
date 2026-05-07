import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DownloadItem, ProgressPayload } from "../types/downloads";

const hydrateDownload = (download: DownloadItem): DownloadItem => ({
    ...download,
    verified_speed: download.protocol === "torrent" ? 0 : download.speed,
    status_text:
        download.status === "error"
            ? download.status_text ?? download.error_message ?? "Download failed"
            : download.status === "downloading"
                ? download.status_text ?? (download.protocol === "torrent" ? "Restoring session..." : "Connecting...")
                : download.status_text,
    status_phase:
        download.status === "downloading"
            ? download.status_phase ?? (download.protocol === "torrent" ? "restoring_session" : "connecting")
            : download.status_phase,
    phase_elapsed_secs:
        download.status === "downloading"
            ? download.phase_elapsed_secs ?? 0
            : download.phase_elapsed_secs,
});

export function useDownloads() {
    const [downloads, setDownloads] = useState<DownloadItem[]>([]);
    const [autocatchUrl, setAutocatchUrl] = useState("");
    const hasAutoResumed = useRef(false);
    const hasStartupReconciled = useRef(false);

    const markRestoring = useCallback((id: string) => {
        setDownloads((prev) =>
            prev.map((item) =>
                item.id === id
                    ? {
                        ...item,
                        status: "downloading",
                        status_text: "Restoring session...",
                        status_phase: "restoring_session",
                        phase_elapsed_secs: 0,
                    }
                    : item
            )
        );
    }, []);

    const refreshDownloads = useCallback(async () => {
        try {
            const [downloads, settings] = await Promise.all([
                invoke<DownloadItem[]>("get_downloads"),
                invoke<{ auto_resume?: string }>("get_settings"),
            ]);

            setDownloads(downloads.map(hydrateDownload));

            if (settings.auto_resume === "true" && !hasAutoResumed.current) {
                hasAutoResumed.current = true;
                for (const download of downloads) {
                    if (download.status === "downloading") {
                        markRestoring(download.id);
                        await invoke("resume_download", { id: download.id }).catch(console.error);
                        await new Promise((resolve) => setTimeout(resolve, 250));
                    }
                }
            }

            if (!hasStartupReconciled.current && settings.auto_resume !== "true") {
                hasStartupReconciled.current = true;
                const staleActive = downloads.filter((download) => download.status === "downloading");
                for (const download of staleActive) {
                    markRestoring(download.id);
                    await invoke("resume_download", { id: download.id }).catch(console.error);
                    await new Promise((resolve) => setTimeout(resolve, 250));
                }
            }
        } catch (err) {
            console.error("Failed to fetch downloads:", err);
        }
    }, [markRestoring]);

    useEffect(() => {
        refreshDownloads();

        const unlistenProgress = listen<ProgressPayload>("download-progress", (event) => {
            const progress = event.payload;
            setDownloads((prev) =>
                prev.map((download) => {
                    if (download.id !== progress.id) return download;
                    if (download.status === "completed") return download;

                    const total = Math.max(progress.total, 0);
                    const downloaded =
                        total > 0
                            ? Math.min(Math.max(progress.downloaded, 0), total)
                            : Math.max(progress.downloaded, 0);
                    const networkReceivedRaw = progress.network_received ?? progress.downloaded;
                    const networkReceived =
                        total > 0
                            ? Math.min(Math.max(networkReceivedRaw, downloaded), total)
                            : Math.max(networkReceivedRaw, downloaded);

                    return {
                        ...download,
                        downloaded,
                        network_received: networkReceived,
                        verified_speed: progress.verified_speed ?? progress.speed,
                        size: total,
                        speed: progress.speed,
                        eta: progress.eta,
                        connections: progress.connections,
                        status: progress.status_text === "Paused" || progress.status_phase === "paused" ? "paused" : "downloading",
                        status_text: progress.status_text,
                        status_phase: progress.status_phase,
                        phase_elapsed_secs: progress.phase_elapsed_secs,
                    };
                })
            );
        });

        const unlistenCompleted = listen<string>("download-completed", () => {
            refreshDownloads();
        });

        const unlistenName = listen<{ id: string; filename: string }>("download-name-updated", (event) => {
            setDownloads((prev) =>
                prev.map((download) =>
                    download.id === event.payload.id
                        ? { ...download, filename: event.payload.filename }
                        : download
                )
            );
        });

        const unlistenAutocatch = listen<string>("autocatch-url", async (event) => {
            try {
                const settings = await invoke<Record<string, string>>("get_settings");
                if (settings.autocatch_enabled === "true") {
                    setAutocatchUrl(event.payload);
                }
            } catch (err) {
                console.error("Failed to check autocatch setting:", err);
            }
        });

        const unlistenError = listen<{ id: string; message: string }>("download-error", (event) => {
            setDownloads((prev) =>
                prev.map((download) =>
                    download.id === event.payload.id
                        ? { ...download, status: "error", status_text: event.payload.message }
                        : download
                )
            );
        });

        return () => {
            unlistenProgress.then((unlisten) => unlisten());
            unlistenCompleted.then((unlisten) => unlisten());
            unlistenName.then((unlisten) => unlisten());
            unlistenAutocatch.then((unlisten) => unlisten());
            unlistenError.then((unlisten) => unlisten());
        };
    }, [refreshDownloads]);

    return {
        autocatchUrl,
        downloads,
        refreshDownloads,
        setDownloads,
    };
}
