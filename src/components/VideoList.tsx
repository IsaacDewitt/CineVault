import type { VideoWithTags } from "../types";
import { VideoCard, useVideoCardMenu } from "./VideoCard";
import { Icon } from "./icons";

export function VideoList({
  videos,
  onOpen,
  onPlay,
  onChanged,
  loading = false,
}: {
  videos: VideoWithTags[];
  onOpen: (v: VideoWithTags) => void;
  onPlay: (v: VideoWithTags) => void;
  onChanged: () => void;
  loading?: boolean;
}) {
  const { openMenu, menuNode } = useVideoCardMenu({ onOpen, onPlay, onChanged });

  if (videos.length === 0) {
    return (
      <div className={`empty${loading ? " loading" : ""}`}>
        <div className="empty-icon">
          <Icon name={loading ? "refresh" : "film"} size={28} />
        </div>
        <p>{loading ? "正在读取影库…" : "没有符合条件的视频"}</p>
        <span>{loading ? "本地数据库读取中" : "换个关键词或放宽筛选条件试试"}</span>
      </div>
    );
  }

  return (
    <div className="video-grid">
      {videos.map((v) => (
        <VideoCard
          key={v.id}
          video={v}
          onOpen={() => onOpen(v)}
          onPlay={() => onPlay(v)}
          onContextMenu={(e) => openMenu(e, v)}
        />
      ))}
      {menuNode}
    </div>
  );
}
