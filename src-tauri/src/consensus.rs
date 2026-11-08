// ============================================
// 目录共识命名（Directory Consensus Naming）
// ============================================
// 解决的问题：分类器是「逐文件独立」的——剧集名只来自 clean_series_name(单个文件名)，
// 同一目录下每集的发布组标记不同（WEB EDITH / HMAX / NTb / eztv / xingfan cc），
// 于是每集各起一个剧集名，一个文件夹被拆成 N 个剧集。
//
// 本模块在扫描写库前跑一趟「预处理」：按目录内**词级共识**推断该目录的剧集名，
// 给同组文件统一指定归属。本质是补上分类器缺的那一环：
//   分类器能判「这堆文件是剧集」，却判不出「它们是同一个剧集」。
//
// 设计原则：**只做加法，不改分类器**。
//   - 分类器照常跑，输出照常
//   - 本模块只在「同一目录内多文件共享足够多的词」时，覆盖剧集名
//   - 判不准就返回 None，一切照旧（宁可拆散，不可串记录）

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use crate::classifier;

/// 组内最少文件数：少于这个数不做共识（单文件无「共识」可言）
pub const MIN_GROUP_SIZE: usize = 3;

/// 共识阈值：该词出现在组内 ≥ 此比例的文件中，才算「共识词」
const CONSENSUS_RATIO: f32 = 0.6;

/// 共识词最少数量：少于这个数说明文件名差异太大，不敢认（宁可拆散）
const MIN_CONSENSUS_WORDS: usize = 2;

/// 分辨率/画质词表：这些词永远不进剧集名
static NOISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)^(?:
            1080p|720p|480p|2160p|4k|8k|uhd|hd|sd|fullhd
          | x26[45]|h26[45]|hevc|avc|av1|10bit|8bit
          | bluray|blu-ray|bdrip|brrip|webrip|web-dl|webdl|web|hdtv|dvdrip|dvd|hmax|max
          | aac|ac3|eac3|ddp?[0-9.]*|dts(?:-hd)?(?:-ma)?|flac|truehd|atmos|2audio|dual
          | remux|proper|repack|internal|complete|extended
          | chs|cht|eng|chi|gb|big5|中字|中英|双语|字幕|国语|粤语|简繁|繁体|简体
          | www|com|cc|net|org|re|rip|v[0-9]+
          | s[0-9]+|e[0-9]+|season|episode|part[0-9]*
        )$",
    )
    .unwrap()
});

/// 是否噪声词（分辨率/编码/音频/语言/季集标记/站点名等），不参与共识
fn is_noise(word: &str) -> bool {
    NOISE_RE.is_match(word)
}

/// 发布组/站点标记的结构化淘汰。
///
/// 两个信号联合判断，且**职责分明**：
///   ① **覆盖率**（主判据）：剧集本体名词在每组文件里都出现（覆盖 100%）；
///      发布组标记只覆盖部分文件（EDITH 那 20 集有、sin 那 3 集用的是 xingfan 后缀）。
///   ② **相对位置**（仅作缺额的加权）：位置**只在覆盖率有缺口时**才施加惩罚。
///
/// 关键设计（踩坑记录）：初版让位置无条件参与打分，结果 100% 覆盖的 superman
/// （avgpos 0.596，比 my 靠后）被自己的位置惩罚压到阈值以下，剧集名退化成 "my adventures"。
/// 正确做法是「覆盖率是主、位置只解释缺额」：
///   penalty = (1 - cover) × 与该词位置无关的系数
/// 即**覆盖率满分的词永远零惩罚**，覆盖率有缺口的词才按位置远近来排序。
///
/// 代入本次事故真实数据（23 文件）：
///   my/adventures/with/superman → cover 1.000 → penalty 0
///   edith → cover 0.870 → penalty > 0 → 落选
fn trim_release_marks(words: &[String], titles: &[String]) -> Vec<String> {
    if words.len() < 2 || titles.is_empty() {
        return words.to_vec();
    }
    let total = titles.len() as f32;

    struct Stat {
        word: String,
        cover: f32,
        avg_pos: f32,
    }

    let mut stats: Vec<Stat> = Vec::new();
    for w in words {
        let mut hits = 0usize;
        let mut pos_sum = 0.0_f32;
        for t in titles {
            let toks = tokenize(t);
            if let Some(i) = toks.iter().position(|x| x == w) {
                hits += 1;
                pos_sum += i as f32 / toks.len().max(1) as f32;
            }
        }
        stats.push(if hits == 0 {
            Stat { word: w.clone(), cover: 0.0, avg_pos: 1.0 }
        } else {
            Stat {
                word: w.clone(),
                cover: hits as f32 / total,
                avg_pos: pos_sum / hits as f32,
            }
        });
    }

    let max_cover = stats.iter().map(|s| s.cover).fold(0.0_f32, f32::max);
    if max_cover <= 0.0 {
        return words.to_vec();
    }

    // 覆盖率满分（= 与最高覆盖持平）的词：零惩罚，全部保留。
    // 有缺口的词：按「缺额 × 位置靠后程度」排序，缺口越大、位置越靠后，越可能落选。
    let max_pos = stats.iter().map(|s| s.avg_pos).fold(0.0_f32, f32::max).max(1e-6);
    let score = |s: &Stat| {
        let gap = (1.0 - s.cover / max_cover).max(0.0); // 0 = 覆盖满，越大缺口越大
        // gap == 0 → 无惩罚
        let pos_factor = s.avg_pos / max_pos; // 0..1，越靠后越接近 1
        s.cover - gap * pos_factor * 0.5
    };

    let best = stats.iter().map(score).fold(f32::MIN, f32::max);
    let cutoff = best * 0.95;
    let kept: Vec<String> = stats
        .iter()
        .filter(|s| score(s) >= cutoff)
        .map(|s| s.word.clone())
        .collect();

    if kept.len() < MIN_CONSENSUS_WORDS {
        return Vec::new();
    }
    words.iter().filter(|w| kept.contains(w)).cloned().collect()
}

/// 把一个文件名切成「词」：先用分类器的 clean_series_name 归一化，
/// 再按空白切分、小写化、丢噪声词。
/// 注意：clean_series_name 只删已知后缀，发布组名（EDITH/NTb/xingfan）会留下——
/// 这正是我们要的：它们因每集不同而通不过共识阈值，会被自动淘汰。
fn tokenize(title: &str) -> Vec<String> {
    classifier::clean_series_name(title)
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && !is_noise(w))
        .collect()
}

/// 对一组同目录文件的标题做词频共识，返回共识词（按「首现顺序」排列，便于还原剧集名语序）
fn consensus_words(titles: &[String]) -> Vec<String> {
    if titles.len() < MIN_GROUP_SIZE {
        return Vec::new();
    }
    let threshold = (titles.len() as f32 * CONSENSUS_RATIO).ceil() as usize;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    // 每个文件只计一次（同文件内重复词不叠加）
    let tokenized: Vec<Vec<String>> = titles.iter().map(|t| tokenize(t)).collect();
    for toks in &tokenized {
        let mut seen_this: HashMap<&str, ()> = HashMap::new();
        for w in toks {
            if seen_this.insert(w.as_str(), ()).is_none() {
                *counts.entry(w.as_str()).or_insert(0) += 1;
            }
        }
    }
    for toks in &tokenized {
        for w in toks {
            if counts.get(w.as_str()).copied().unwrap_or(0) >= threshold && !order.contains(w) {
                order.push(w.clone());
            }
        }
    }
    order
}

/// 用共识词拼剧集名。
/// 语序还原：按「该词在组内首次出现的词序位置」的中位数排列，
/// 这样 "My Adventures with Superman" 不会被拼成 "Adventures My Superman with"。
fn build_name_from_consensus(consensus: &[String], titles: &[String]) -> Option<String> {
    if consensus.len() < MIN_CONSENSUS_WORDS {
        return None;
    }
    // 词 → 位置样本
    let mut pos: HashMap<String, Vec<usize>> = HashMap::new();
    for t in titles {
        for (i, w) in tokenize(t).iter().enumerate() {
            if consensus.contains(w) {
                pos.entry(w.clone()).or_default().push(i);
            }
        }
    }
    let mut ranked: Vec<(f32, String)> = consensus
        .iter()
        .map(|w| {
            let v = pos.get(w).cloned().unwrap_or_default();
            let med = if v.is_empty() {
                f32::MAX
            } else {
                let mut s = v.clone();
                s.sort_unstable();
                s[s.len() / 2] as f32
            };
            (med, w.clone())
        })
        .collect();
    ranked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let words: Vec<String> = ranked.into_iter().map(|(_, w)| w).collect();
    if words.len() < MIN_CONSENSUS_WORDS {
        return None;
    }
    Some(words.join(" "))
}

/// 目录共识结果：目录 → 该目录的剧集名
pub type DirectoryNames = HashMap<PathBuf, String>;

/// 方案 A 的启用开关（**默认关闭**）。
///
/// 2026-09-19 决策：真实生产库回放发现，本算法在「广告水印污染」目录上会把名字改坏。
/// 例：`大明王朝` 目录 → `mpan com]d明w朝1566`；`三国演义.动画版` →
/// `hd中字无水印[最新电影www dygangs me`；`狂飙` → `punch out yyds`。
/// 根因：国配资源文件名常带广告水印前缀（`hd中字无水印[最新电影www.xxx.me]`），
/// 这些串在**每个文件里都出现**，覆盖率 100%，被算法当成最可靠的词提拔成剧集名。
/// 覆盖率判据无法区分「共有的正确信息」与「共有的垃圾」。
///
/// 因此**暂不启用**：防拆散的任务由方案 C（扫描后自动收紧 folder_path 锚点）承担，
/// 它只补锚点、不改名，且只作用于无锚点剧集（生产库 99 个剧集里仅 1 个无锚点）。
///
/// 若日后要启用，前提是先解决水印污染（把广告串纳入噪声词表并全库回放验证）。
/// 启用方式：把此常量改为 true。
pub const ENABLE_DIRECTORY_CONSENSUS: bool = false;

/// 是否启用目录共识命名（见 ENABLE_DIRECTORY_CONSENSUS 说明）
pub fn is_enabled() -> bool {
    ENABLE_DIRECTORY_CONSENSUS
}

/// 测试专用入口：绕过开关，直测算法核心。
/// 生产代码**不要**调用此函数——它无视 ENABLE_DIRECTORY_CONSENSUS，
/// 用途仅限于验证「开关一旦打开，算法本身是否正确」。
#[cfg(test)]
pub fn directory_series_names_ungated_for_test(files: &[PathBuf]) -> DirectoryNames {
    compute_directory_names(files)
}

/// 扫描结果集 → 目录共识剧集名。
/// 只对「同一目录下 ≥ MIN_GROUP_SIZE 个视频」的目录产出条目；
/// 其余目录不出现在结果里，调用方原样走分类器。
///
/// **开关关闭时直接返回空表**（见 ENABLE_DIRECTORY_CONSENSUS 的说明）——
/// 这样调用方无需分叉逻辑，恒等得出 None，行为与改动前完全一致。
///
/// 注意：本函数**不做剧集/电影判定**，只负责「同目录这批文件该叫什么名字」。
/// 是不是剧集仍由分类器 + kind 决定（见调用处）。
pub fn directory_series_names(files: &[PathBuf]) -> DirectoryNames {
    if !is_enabled() {
        return DirectoryNames::new();
    }
    compute_directory_names(files)
}

/// 算法本体（不含开关判断）。
fn compute_directory_names(files: &[PathBuf]) -> DirectoryNames {
    let mut out = DirectoryNames::new();
    // 按父目录分组
    let mut groups: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for f in files {
        if let Some(parent) = f.parent() {
            let title = f.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if !title.is_empty() {
                groups.entry(parent.to_path_buf()).or_default().push(title);
            }
        }
    }

    for (dir, titles) in groups {
        if titles.len() < MIN_GROUP_SIZE {
            continue;
        }
        let consensus = consensus_words(&titles);
        let core = trim_release_marks(&consensus, &titles);
        if let Some(name) = build_name_from_consensus(&core, &titles) {
            out.insert(dir, name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// 测试专用：绕过 ENABLE_DIRECTORY_CONSENSUS 开关，直接跑算法核心。
    /// 生产路径经 directory_series_names（开关关闭时返回空表），
    /// 算法本身的正确性在此独立验证——这样开关一开就是验证过的行为。
    fn names_ungated(files: &[PathBuf]) -> DirectoryNames {
        compute_directory_names(files)
    }

    /// 生产路径的开关必须默认关闭：这是 2026-09-19 基于真实数据回放的决定
    #[test]
    fn consensus_is_disabled_by_default() {
        assert!(
            !is_enabled(),
            "方案 A 默认必须关闭：真实库回放显示水印污染目录会被改坏名（见常量注释）"
        );
        let files: Vec<PathBuf> = superman_titles().iter().map(|s| p(s)).collect();
        assert!(
            directory_series_names(&files).is_empty(),
            "开关关闭时生产入口必须返回空表，行为与改动前一致"
        );
    }

    /// 本次事故的真实样本：27 集，四种发布组标记混在一起
    fn superman_titles() -> Vec<String> {
        let mut v = Vec::new();
        for e in 1..=10 {
            v.push(format!("H:\\待看的剧\\我与超人冒险\\My.Adventures.with.Superman.S01E{:02}.1080p.WEB.h264-EDITH.chs.eng", e));
        }
        for e in 1..=10 {
            v.push(format!("H:\\待看的剧\\我与超人冒险\\My.Adventures.with.Superman.S02E{:02}.1080p.WEB.h264-EDITH.chs.eng", e));
        }
        v.push("H:\\待看的剧\\我与超人冒险\\我与超人的冒险.My.Adventures.with.Superman.S03E01.1080p.CHS&ENG.www.xingfan.cc.mp4".into());
        v.push("H:\\待看的剧\\我与超人冒险\\我与超人的冒险.My.Adventures.with.Superman.S03E02.1080p.CHS&ENG.www.xingfan.cc.mp4".into());
        v.push("H:\\待看的剧\\我与超人冒险\\我与超人的冒险.My.Adventures.with.Superman.S03E03.1080p.CHS&ENG.www.xingfan.cc.mp4".into());
        v
    }

    #[test]
    fn consensus_collapses_release_groups() {
        let files: Vec<PathBuf> = superman_titles().iter().map(|s| p(s)).collect();
        let names = names_ungated(&files);
        let name = names
            .get(&p("H:\\待看的剧\\我与超人冒险"))
            .expect("应产出共识名");
        // 核心断言：发布组标记（edith / xingfan / www / cc）一个都不能进名字
        for bad in ["edith", "xingfan", "www", "cc", "1080p", "web"] {
            assert!(!name.contains(bad), "噪声词 {} 混进剧集名: {}", bad, name);
        }
        // 语序：my 应排在 superman 前面
        let lower = name.to_lowercase();
        assert!(lower.contains("adventures"), "got {}", name);
        assert!(lower.contains("superman"), "got {}", name);
        assert!(
            lower.find("adventures").unwrap() < lower.find("superman").unwrap(),
            "词序错乱: {}",
            name
        );
    }

    #[test]
    fn single_file_dir_has_no_consensus() {
        let files = vec![p("H:\\电影\\随便一部电影.2020.1080p.mkv")];
        assert!(names_ungated(&files).is_empty());
    }

    #[test]
    fn dissimilar_files_have_no_consensus() {
        // 同一目录放了几部不相关的剧：共识词太少 → 不该硬凑一个名字
        let files = vec![
            p("H:\\混合\\Breaking.Bad.S01E01.1080p.mkv"),
            p("H:\\混合\\Game.of.Thrones.S01E01.1080p.mkv"),
            p("H:\\混合\\The.Wire.S01E01.1080p.mkv"),
        ];
        let names = names_ungated(&files);
        // s01e01 / 1080p 是噪声，各有 1 次命中不到阈值 → 无共识词
        assert!(names.is_empty(), "不该硬凑: {:?}", names);
    }

    #[test]
    fn movie_dir_not_named_by_consensus() {
        // 三部同系列电影：本模块会给出共识名，但调用方靠 kind 拦截
        // 这里只验证「共识名本身」可产出（调用方的 kind 判定另测）
        let files = vec![
            p("H:\\电影\\The.Lord.of.the.Rings.2001.1080p.mkv"),
            p("H:\\电影\\The.Lord.of.the.Rings.2002.1080p.mkv"),
            p("H:\\电影\\The.Lord.of.the.Rings.2003.1080p.mkv"),
        ];
        let names = names_ungated(&files);
        assert_eq!(names.len(), 1);
    }

    /// 安全红线回归测试：bucket 型电影目录（几十部**不相关**电影堆在一个目录）
    /// 不得被共识成一个剧集名。生产库实测此类目录 37 个，最大的有 156 部电影
    /// （D:\已阅影音\电影房\7.5-9）。
    ///
    /// 双保险：
    ///   ① 本模块：不相关文件名共识词太少 → 返回 None
    ///   ② 调用方 apply_series：只在 classifier 判 kind=="series" 时才采用共识名，
    ///      这些文件全部 kind=="movie" → 共识名根本不会被读
    /// 此测试守 ①；② 由 commands.rs 的 kind 门禁守。
    #[test]
    fn unrelated_movie_bucket_yields_no_consensus() {
        let files = vec![
            p("D:\\电影\\Avengers.Endgame.2019.1080p.BluRay.mkv"),
            p("D:\\电影\\Parasite.2019.1080p.WEB-DL.mkv"),
            p("D:\\电影\\Inception.2010.1080p.BluRay.mkv"),
            p("D:\\电影\\Interstellar.2014.2160p.WEB-DL.mkv"),
            p("D:\\电影\\Whiplash.2014.1080p.BluRay.mkv"),
            p("D:\\电影\\La.La.Land.2016.1080p.BluRay.mkv"),
        ];
        let names = names_ungated(&files);
        assert!(
            names.is_empty(),
            "不相关电影不该产出共识剧集名: {:?}",
            names
        );
    }

    #[test]
    fn noise_words_filtered() {
        assert!(is_noise("1080p"));
        assert!(is_noise("x264"));
        assert!(is_noise("chs"));
        assert!(is_noise("www"));
        assert!(!is_noise("superman"));
        assert!(!is_noise("adventures"));
    }

    #[test]
    fn tokenize_keeps_unknown_words_for_consensus_stage() {
        // tokenize 只丢「已知噪声」，未知发布组名（EDITH）会保留——
        // 这是刻意的：靠词表列举发布组名永远追不上，改由 trim_trailing_stray 用
        // 「覆盖度 + 位置漂移」结构化淘汰。此处验证分工边界。
        let toks = tokenize("My.Adventures.with.Superman.S01E01.1080p.WEB.h264-EDITH.chs.eng");
        assert!(toks.contains(&"superman".to_string()));
        assert!(toks.contains(&"adventures".to_string()));
        assert!(!toks.iter().any(|w| w == "1080p"), "已知噪声该被丢: {:?}", toks);
        assert!(!toks.iter().any(|w| w == "chs"), "已知噪声该被丢: {:?}", toks);
        assert!(toks.iter().any(|w| w == "edith"), "未知发布组留给结构化阶段: {:?}", toks);
    }

    /// 混合批次：一半文件带 EDITH 发布组、另一半不带（本次事故的真实形态）
    #[test]
    fn partial_release_group_is_trimmed() {
        let mut files: Vec<PathBuf> = Vec::new();
        for e in 1..=10 {
            // 带 EDITH 的 10 个
            files.push(p(&format!(
                "H:\\混合测试\\My.Adventures.with.Superman.S01E{:02}.1080p.WEB.h264-EDITH.chs.eng.mp4",
                e
            )));
        }
        for e in 1..=10 {
            // 不带 EDITH 的 10 个（干净发布）
            files.push(p(&format!(
                "H:\\混合测试\\My.Adventures.with.Superman.S02E{:02}.1080p.WEB.h264.chs.eng.mp4",
                e
            )));
        }
        let names = names_ungated(&files);
        let name = names.get(&p("H:\\混合测试")).expect("应产出共识名");
        assert!(!name.contains("edith"), "发布组污染: {}", name);
        assert!(name.contains("superman"), "got {}", name);
    }

    /// 中文名 + 英文名混排（xingfan 那三集带中文前缀）
    #[test]
    fn chinese_prefix_does_not_break_english_core() {
        let files = vec![
            p("H:\\中文混排\\我与超人的冒险.My.Adventures.with.Superman.S01E01.1080p.mp4"),
            p("H:\\中文混排\\我与超人的冒险.My.Adventures.with.Superman.S01E02.1080p.mp4"),
            p("H:\\中文混排\\My.Adventures.with.Superman.S01E03.1080p.mp4"),
            p("H:\\中文混排\\My.Adventures.with.Superman.S01E04.1080p.mp4"),
        ];
        let names = names_ungated(&files);
        let name = names.get(&p("H:\\中文混排")).expect("应产出共识名");
        // 中文只覆盖 50%（2/4），低于 60% 阈值 → 不进共识；英文本体名覆盖 100%
        assert!(name.contains("superman"), "got {}", name);
        assert!(!name.contains("1080p"), "got {}", name);
    }
}
