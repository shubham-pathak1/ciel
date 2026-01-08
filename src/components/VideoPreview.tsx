import React from 'react';
import { Play, Download, Music, Monitor } from 'lucide-react';

interface VideoFormat {
    format_id: string;
    extension: string;
    resolution: string;
    filesize: number | null;
    protocol: string;
    note: string | null;
    acodec: string | null;
    vcodec: string | null;
}

interface VideoMetadata {
    title: string;
    thumbnail: string;
    duration: number | null;
    formats: VideoFormat[];
    url: string;
}

interface VideoPreviewProps {
    metadata: VideoMetadata;
    onDownload: (formatId: string, ext: string, acodec?: string, totalSize?: number) => void;
    onCancel: () => void;
}

export const VideoPreview: React.FC<VideoPreviewProps> = ({ metadata, onDownload, onCancel }) => {
    const [selectedFormat, setSelectedFormat] = React.useState<string | null>(null);

    // Filter useful formats (avoid m3u8 if possible, prefer progressive or dash with sizes)
    const filteredFormats = metadata.formats.filter(f =>
        (f.resolution !== 'audio only' || f.filesize) &&
        !f.protocol.includes('m3u8') &&
        f.extension !== 'mhtml' &&
        f.extension !== 'webm'
    ).sort((a, b) => {
        // Sort by resolution (naive)
        const getRes = (r: string) => parseInt(r.split('x')[1]) || 0;
        return getRes(b.resolution) - getRes(a.resolution);
    });

    const formatSize = (bytes: number | null) => {
        if (!bytes) return 'Unknown size';
        const units = ['B', 'KB', 'MB', 'GB'];
        let size = bytes;
        let unitIndex = 0;
        while (size >= 1024 && unitIndex < units.length - 1) {
            size /= 1024;
            unitIndex++;
        }
        return `${size.toFixed(1)} ${units[unitIndex]}`;
    };

    // Find audio options
    const audioFormats = metadata.formats.filter(f => f.resolution === 'audio only' && f.filesize);
    const bestAudio = audioFormats.sort((a, b) => (b.filesize || 0) - (a.filesize || 0))[0];
    const ecoAudio = audioFormats.sort((a, b) => (a.filesize || 0) - (b.filesize || 0))[0];

    const getTargetAudio = (height: number) => {
        if (height <= 480) return ecoAudio || bestAudio;
        return bestAudio;
    };

    return (
        <div className="space-y-4">
            <div className="flex gap-4">
                {metadata.thumbnail && (
                    <div className="relative w-40 h-24 flex-shrink-0 bg-zinc-900 rounded-lg overflow-hidden border border-zinc-800">
                        <img src={metadata.thumbnail} alt={metadata.title} className="w-full h-full object-cover" />
                        {metadata.duration && (
                            <div className="absolute bottom-1 right-1 bg-black/80 px-1 rounded text-[10px] text-zinc-300 font-mono">
                                {Math.floor(metadata.duration / 60)}:{Math.floor(metadata.duration % 60).toString().padStart(2, '0')}
                            </div>
                        )}
                    </div>
                )}
                <div className="flex-1 min-w-0">
                    <h3 className="text-zinc-100 font-medium truncate leading-tight mb-1" title={metadata.title}>
                        {metadata.title}
                    </h3>
                    <p className="text-zinc-500 text-xs truncate mb-2">{metadata.url}</p>
                    <div className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-zinc-900 border border-zinc-800 text-[10px] text-zinc-400">
                        <Play className="w-3 h-3" />
                        Video Stream Detected
                    </div>
                </div>
            </div>

            <div className="space-y-2">
                <label className="text-xs font-medium text-zinc-400 uppercase tracking-wider">Select Quality</label>
                <div className="max-h-48 overflow-y-auto pr-1 space-y-1 custom-scrollbar">
                    {filteredFormats.map((f, i) => {
                        const height = parseInt(f.resolution.split('x')[1]) || 0;
                        const needsAudio = !f.acodec || f.acodec === 'none';
                        const targetAudio = getTargetAudio(height);
                        const totalSize = (f.filesize || 0) + (needsAudio && targetAudio ? (targetAudio.filesize || 0) : 0);

                        return (
                            <button
                                key={`${f.format_id}-${i}`}
                                onClick={() => setSelectedFormat(f.format_id.toString())}
                                className={`w-full flex items-center justify-between p-2 rounded-lg border transition-all ${selectedFormat === f.format_id
                                    ? 'bg-zinc-100 border-zinc-100 text-zinc-900'
                                    : 'bg-zinc-950 border-zinc-800 text-zinc-400 hover:border-zinc-700 hover:text-zinc-300'
                                    }`}
                            >
                                <div className="flex items-center gap-3">
                                    {f.resolution === 'audio only' ? <Music className="w-4 h-4" /> : <Monitor className="w-4 h-4" />}
                                    <div className="text-left">
                                        <div className={`text-xs font-semibold ${selectedFormat === f.format_id ? 'text-zinc-900' : 'text-zinc-200'}`}>
                                            {f.resolution} {f.note ? `(${f.note})` : ''}
                                        </div>
                                        <div className="text-[10px] opacity-70 capitalize">{f.extension} • {f.protocol}</div>
                                    </div>
                                </div>
                                <div className="text-[10px] font-mono whitespace-nowrap">
                                    {formatSize(totalSize)}
                                </div>
                            </button>
                        );
                    })}
                </div>
            </div>

            <div className="flex gap-2 pt-2">
                <button
                    onClick={onCancel}
                    className="flex-1 px-4 py-2 rounded-lg bg-zinc-900 text-zinc-400 text-sm font-medium hover:bg-zinc-800 transition-colors"
                >
                    Cancel
                </button>
                <button
                    disabled={!selectedFormat}
                    onClick={() => {
                        const f = filteredFormats.find(x => x.format_id === selectedFormat);
                        if (f) {
                            const height = parseInt(f.resolution.split('x')[1]) || 0;
                            const needsAudio = !f.acodec || f.acodec === 'none';
                            const targetAudio = getTargetAudio(height);
                            // Pass audio ID if we need to fetch it separately
                            const audioId = needsAudio && targetAudio ? targetAudio.format_id : undefined;

                            // Calculate total size for backend tracking
                            const totalSize = (f.filesize || 0) + (needsAudio && targetAudio ? (targetAudio.filesize || 0) : 0);

                            onDownload(f.format_id.toString(), f.extension.toString(), audioId, totalSize);
                        }
                    }}
                    className="flex-[2] flex items-center justify-center gap-2 px-4 py-2 rounded-lg bg-zinc-100 text-zinc-950 text-sm font-bold hover:bg-white transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                >
                    <Download className="w-4 h-4" />
                    Download Selected
                </button>
            </div>
        </div>
    );
};
