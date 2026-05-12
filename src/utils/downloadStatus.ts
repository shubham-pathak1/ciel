import type { DownloadItem } from "../types/downloads";
import { formatPhaseElapsed } from "./downloadFormatting";

export const getFriendlyErrorMessage = (message?: string | null) => {
    const raw = (message ?? "").trim();
    const lower = raw.toLowerCase();

    if (!raw) return "Something went wrong. Try again.";
    if (
        lower.includes("video-downloads.googleusercontent.com") &&
        (lower.includes("error sending request for url") ||
            lower.includes("connection reset") ||
            lower.includes("timed out") ||
            lower.includes("timeout"))
    ) {
        return "This Google media link is temporary and can fail even when fresh. Open the source page again, copy a new direct file URL, then retry with cookies or a browser User-Agent if needed.";
    }
    if (lower.includes("error sending request for url")) {
        return "Could not connect to the download host. The link may be temporary, blocked by the server, or require cookies/User-Agent.";
    }
    if (lower.includes("connection stalled")) {
        return "The server stopped sending data. Retry, or lower parallel connections for this host.";
    }
    if (lower.includes("invalid url")) return "That link does not look valid. Check it and try again.";
    if (lower.includes("html") || lower.includes("text/html") || lower.includes("<!doctype") || lower.includes("<html")) {
        return "This server returned a web page instead of the file. It may need login cookies or a browser download link.";
    }
    if (lower.includes("403") || lower.includes("forbidden")) {
        return "Access was blocked by the server. Try cookies, a browser User-Agent, or a fresh link.";
    }
    if (lower.includes("401") || lower.includes("unauthorized")) {
        return "This download needs authentication. Add cookies or sign in and copy a fresh link.";
    }
    if (lower.includes("404") || lower.includes("not found")) return "The file was not found. The link may be expired or moved.";
    if (lower.includes("416") || lower.includes("range")) {
        return "The server rejected resume ranges. Restarting as a single-connection download may work.";
    }
    if (lower.includes("timed out") || lower.includes("timeout")) return "The connection timed out. Check the network and retry.";
    if (lower.includes("failed to read .torrent")) return "Ciel could not read that .torrent file. Choose the file again or check its permissions.";
    if (lower.includes("metadata") && (lower.includes("torrent") || lower.includes("magnet"))) {
        return "Torrent metadata could not be loaded. The magnet may be stale, or peers/trackers are not responding.";
    }
    if (lower.includes("torrent engine is still initializing")) {
        return "Torrent engine is still starting. Wait a few seconds and try again.";
    }
    if (lower.includes("no files are selected")) return "No files are selected for this torrent.";
    if (raw.length > 150) return `${raw.slice(0, 147)}...`;
    return raw.replace(/^error:\s*/i, "");
};

export const getPhaseHint = (phase?: string) => {
    switch (phase) {
        case "restoring_session":
            return "restoring";
        case "resuming":
            return "resuming";
        case "restarting":
            return "restarting";
        case "verifying_data":
            return "checking files";
        case "finding_peers":
            return "searching peers";
        case "connecting":
            return "connecting";
        case "negotiating_peers":
            return "negotiating peers";
        case "preparing_first_piece":
            return "receiving data";
        case "fetching_metadata":
            return "loading metadata";
        case "fallback_single":
            return "single connection";
        default:
            return "";
    }
};

export const getTorrentPhaseDisplay = (download: DownloadItem) => {
    const phase = download.status_phase;
    const peers = Math.max(download.connections ?? 0, 0);
    const elapsed = formatPhaseElapsed(download.phase_elapsed_secs);
    const suffix = elapsed ? ` · ${elapsed}` : "";

    switch (phase) {
        case "fetching_metadata":
            return {
                title: "Reading torrent metadata",
                detail: peers > 0 ? `Connected to ${peers} peer${peers === 1 ? "" : "s"}${suffix}` : `Waiting for metadata${suffix}`,
            };
        case "initializing":
            return {
                title: "Preparing torrent",
                detail: `Setting up the download${suffix}`,
            };
        case "connecting":
            return {
                title: "Finding peers",
                detail: `Looking for available peers${suffix}`,
            };
        case "negotiating_peers":
            return {
                title: "Connecting to peers",
                detail: peers > 0 ? `${peers} peer${peers === 1 ? "" : "s"} found${suffix}` : `Waiting for the swarm${suffix}`,
            };
        case "preparing_first_piece":
            return {
                title: "Starting download",
                detail: "Receiving the first verified data",
            };
        case "verifying_data":
            return {
                title: "Checking existing files",
                detail: download.status_text?.replace(/^Verifying local data\.\.\.\s*/i, "") || `Verifying local data${suffix}`,
            };
        case "restoring_session":
            return {
                title: "Restoring torrent",
                detail: `Reconnecting saved session${suffix}`,
            };
        case "resuming":
            return {
                title: "Resuming download",
                detail: `Reconnecting transfer${suffix}`,
            };
        case "restarting":
            return {
                title: "Restarting download",
                detail: "This server does not support resume",
            };
        default:
            return null;
    }
};

export const isLocalTorrentPath = (value: string) => {
    const trimmed = value.trim();
    if (!trimmed) return false;
    if (/^[a-z]+:\/\//i.test(trimmed) || trimmed.startsWith("magnet:")) return false;
    return /\.torrent$/i.test(trimmed);
};

export const isHtmlResponse = (typeInfo: { content_type?: string | null }) =>
    typeInfo.content_type?.toLowerCase().includes("text/html") ?? false;
