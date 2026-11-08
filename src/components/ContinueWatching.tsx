import type { VideoWithTags } from "../types";
import { VideoCard, useVideoCardMenu } from "./VideoCard";
import { Icon } from "./icons";

export function ContinueWatching({
  videos,
  onOpen,
  onPlay,
  onChanged,
}: {
  videos: VideoWithTags[];
  onOpen: (v: VideoWithTags) => void;
  onPlay: (v: VideoWithTags) => void;
  onChanged: () => void;
}) {
  // 与主列表共用同一套右键菜单（播放 / 打开文件夹 / 详情 / 评分 / 进度 / 删除）。
  const { openMenu, menuNode } = useVideoCardMenu({ onOpen, onPlay, onChanged });

  if (videos.length === 0) return null;

  return (
    <>
      <section className="continue-section" aria-labelledby="continue-title">
        <div className="section-heading">
          <div className="section-heading-title">
            <span className="section-kicker">
              <Icon name="clock" size={13} />
              CONTINUE
            </span>
            <h2 id="continue-title">继续观看</h2>
          </div>
          <span className="section-hint">从上次停下的位置接着看</span>
        </div>
        <div className="continue-rail">
          {videos.map((v) => (
            <VideoCard
              key={v.id}
              video={v}
              onOpen={() => onOpen(v)}
              onPlay={() => onPlay(v)}
              onContextMenu={(e) => openMenu(e, v)}
            />
          ))}
        </div>
      </section>
      {/* 菜单与弹窗放在横向滚动轨道之外，避免被 overflow-x:auto 裁切 */}
      {menuNode}
    </>
  );
}
