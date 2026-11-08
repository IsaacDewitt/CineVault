import type {
  SeriesOverview,
  SortSpec,
  Tag,
  VideoWithTags,
  WatchState,
} from "../types";

export interface RatingRange {
  enabled: boolean;
  min: number;
  max: number;
}

export interface LibraryFilters {
  query: string;
  selectedTagIds: number[];
  watchStates: Set<WatchState>;
  ratingRange: RatingRange;
}

export function hasActiveFilters(filters: LibraryFilters): boolean {
  return (
    filters.query.trim() !== "" ||
    filters.selectedTagIds.length > 0 ||
    filters.watchStates.size < 3 ||
    filters.ratingRange.enabled
  );
}

function queryTerms(query: string): string[] {
  return query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
}

function tagNames(tags: Tag[] | undefined): string[] {
  return (tags ?? []).map((t) => t.name.toLowerCase());
}

/** 关键词搜索与后端同口径：标题 / 文件名 / 剧集名 / 标签，多个词 AND。 */
export function matchesVideoQuery(v: VideoWithTags, query: string): boolean {
  const terms = queryTerms(query);
  if (terms.length === 0) return true;
  const hay = [
    v.title,
    v.file_name,
    v.series_name ?? "",
    ...tagNames(v.tags),
  ]
    .join(" ")
    .toLowerCase();
  return terms.every((term) => hay.includes(term));
}

export function matchesSeriesQuery(s: SeriesOverview, query: string): boolean {
  const terms = queryTerms(query);
  if (terms.length === 0) return true;
  const hay = [s.name, ...tagNames(s.tags)].join(" ").toLowerCase();
  return terms.every((term) => hay.includes(term));
}

function matchesRating(rating: number, range: RatingRange): boolean {
  if (!range.enabled) return true;
  if (range.min > 0 && rating < range.min) return false;
  if (range.max < 10 && rating > range.max) return false;
  return true;
}

function matchesWatchState(progress: number, states: Set<WatchState>): boolean {
  if (states.size >= 3) return true;
  if (states.size === 0) return false;
  const state: WatchState =
    progress >= 99.9 ? "watched" : progress > 0 ? "partial" : "unwatched";
  return states.has(state);
}

function matchesTags(tags: Tag[] | undefined, selectedIds: number[]): boolean {
  if (selectedIds.length === 0) return true;
  const ids = new Set((tags ?? []).map((t) => t.id));
  return selectedIds.every((id) => ids.has(id));
}

export function matchesVideoFilters(
  v: VideoWithTags,
  filters: LibraryFilters
): boolean {
  return (
    matchesVideoQuery(v, filters.query) &&
    matchesTags(v.tags, filters.selectedTagIds) &&
    matchesWatchState(v.watch_progress, filters.watchStates) &&
    matchesRating(v.rating, filters.ratingRange)
  );
}

export function matchesSeriesFilters(
  s: SeriesOverview,
  filters: LibraryFilters
): boolean {
  return (
    matchesSeriesQuery(s, filters.query) &&
    matchesTags(s.tags, filters.selectedTagIds) &&
    matchesWatchState(s.progress, filters.watchStates) &&
    matchesRating(s.rating, filters.ratingRange)
  );
}

function compareNullableText(a: string | null | undefined, b: string | null | undefined): number {
  return (b ?? "").localeCompare(a ?? "");
}

function compareRatings(a: number, b: number, dir: number): number {
  const av = a > 0 ? a : null;
  const bv = b > 0 ? b : null;
  if (av === null && bv === null) return 0;
  if (av === null) return 1;
  if (bv === null) return -1;
  return dir * (av - bv);
}

function compareDurations(
  a: number | null | undefined,
  b: number | null | undefined,
  dir: number
): number {
  const av = a ?? null;
  const bv = b ?? null;
  if (av === null && bv === null) return 0;
  if (av === null) return 1;
  if (bv === null) return -1;
  return dir * (av - bv);
}

/** 与后端 query_videos_paginated 的排序口径保持一致；最近类恒降序。 */
export function sortVideoList(
  videos: VideoWithTags[],
  sort: SortSpec
): VideoWithTags[] {
  const dir = sort.dir === "asc" ? 1 : -1;
  return [...videos].sort((a, b) => {
    switch (sort.key) {
      case "name":
        return dir * a.title.localeCompare(b.title, "zh-Hans-CN");
      case "size":
        return dir * (a.file_size - b.file_size);
      case "rating":
        return compareRatings(a.rating, b.rating, dir);
      case "duration":
        return compareDurations(a.duration, b.duration, dir);
      case "updated":
        return compareNullableText(a.updated_at, b.updated_at);
      case "last_watched":
      default:
        return compareNullableText(a.last_watched_at, b.last_watched_at);
    }
  });
}

export function sortSeriesList(
  series: SeriesOverview[],
  sort: SortSpec
): SeriesOverview[] {
  const dir = sort.dir === "asc" ? 1 : -1;
  return [...series].sort((a, b) => {
    switch (sort.key) {
      case "name":
        return dir * a.name.localeCompare(b.name, "zh-Hans-CN");
      case "size":
        return dir * (a.total_size - b.total_size);
      case "rating":
        return compareRatings(a.rating, b.rating, dir);
      case "duration":
        return compareDurations(a.total_duration, b.total_duration, dir);
      case "updated":
        return compareNullableText(a.updated_at, b.updated_at);
      case "last_watched":
      default:
        return compareNullableText(a.last_watched_at, b.last_watched_at);
    }
  });
}

export function formatDuration(seconds: number | null | undefined): string {
  if (!seconds || !Number.isFinite(seconds) || seconds <= 0) return "";
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}`;
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}
