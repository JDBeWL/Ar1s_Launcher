//! 下载源选择与镜像地址构造
//!
//! BMCL 与 MC 光速镜像（主线路 / EO 线路）共用同一套镜像路径：
//! `/mc/game/version_manifest.json`、`/v1/packages/{sha1}/{name}.json`、
//! `/v1/objects/{sha1}/{name}`、`/assets/{prefix}/{hash}`、`/maven/{path}`。
//! 所以同一个任务可以按「首选源 → 其他国内镜像 → 官方源」生成整条候选链，
//! 单个源失败或卡住时直接换下一个，不用把地址规则散落在各个业务模块里。

use crate::services::config;

pub const BMCL_BASE: &str = "https://bmclapi2.bangbang93.com";
pub const HKMC_MAIN_BASE: &str = "https://bsmirror.hkmc.online";
pub const HKMC_EO_BASE: &str = "https://mirror.hkmc.online";
pub const OFFICIAL_META_BASE: &str = "https://launchermeta.mojang.com";
pub const OFFICIAL_RESOURCES_BASE: &str = "https://resources.download.minecraft.net";
pub const MAVEN_FORGE: &str = "https://maven.minecraftforge.net";
pub const MAVEN_NEOFORGE: &str = "https://maven.neoforged.net/releases";
pub const MAVEN_MINECRAFT: &str = "https://libraries.minecraft.net";

const MIRROR_BASES: [(&str, DownloadSource); 3] = [
    (BMCL_BASE, DownloadSource::Bmcl),
    (HKMC_MAIN_BASE, DownloadSource::HkmcMain),
    (HKMC_EO_BASE, DownloadSource::HkmcEo),
];

/// 一批元数据请求候选：(源, 该源的清单 URL)
type ManifestBatch = Vec<(DownloadSource, String)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadSource {
    /// 自动：同时探测官方与全部镜像，谁先响应就用谁
    Auto,
    /// 优先官方：官方源先试，失败后并发尝试镜像
    Official,
    /// 优先镜像：并发尝试镜像，全部失败后再回退官方
    Mirror,
    /// 已解析出的具体镜像源
    Bmcl,
    HkmcMain,
    HkmcEo,
}

impl DownloadSource {
    /// 配置值 → 下载模式
    ///
    /// 旧版本保存的 `bmcl` / `hkmc` / `hkmc-eo` 都等价于「优先镜像」，
    /// 升级后统一归到 `Mirror`；未知非空值也按镜像处理。
    pub fn from_setting(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("auto") | None | Some("") => Self::Auto,
            Some("official") => Self::Official,
            Some(_) => Self::Mirror,
        }
    }

    pub fn current() -> Self {
        config::load_config()
            .map(|config| Self::from_setting(config.download_mirror.as_deref()))
            .unwrap_or(Self::Auto)
    }

    /// 是否会使用国内镜像（Auto / Mirror 与具体镜像源都算）
    pub fn is_mirror(self) -> bool {
        self != Self::Official
    }

    pub fn base(self) -> Option<&'static str> {
        match self {
            Self::Bmcl => Some(BMCL_BASE),
            Self::HkmcMain => Some(HKMC_MAIN_BASE),
            Self::HkmcEo => Some(HKMC_EO_BASE),
            Self::Auto | Self::Official | Self::Mirror => None,
        }
    }

    /// 当前源下的镜像顺序：具体镜像源优先自己，模式源优先 BMCL
    pub fn mirror_bases(self) -> Vec<&'static str> {
        match self {
            Self::Official => Vec::new(),
            Self::Bmcl | Self::Auto | Self::Mirror => {
                vec![BMCL_BASE, HKMC_MAIN_BASE, HKMC_EO_BASE]
            }
            Self::HkmcMain => vec![HKMC_MAIN_BASE, HKMC_EO_BASE, BMCL_BASE],
            Self::HkmcEo => vec![HKMC_EO_BASE, HKMC_MAIN_BASE, BMCL_BASE],
        }
    }

    /// 元数据请求分两批返回：一批并发竞速，全失败后再跑下一批
    ///
    /// - Auto：官方与镜像一起竞速，谁先返回用谁
    /// - Mirror：镜像之间竞速，全部失败才回退官方
    /// - Official：官方单独先试，失败后再并发尝试镜像
    pub fn manifest_batches(self) -> (ManifestBatch, ManifestBatch) {
        let official = || (Self::Official, manifest_url(Self::Official));
        let mirrors: Vec<(Self, String)> = [Self::Bmcl, Self::HkmcMain, Self::HkmcEo]
            .into_iter()
            .map(|source| (source, manifest_url(source)))
            .collect();

        match self {
            Self::Auto => {
                let mut all = mirrors;
                all.push(official());
                (all, Vec::new())
            }
            Self::Mirror => (mirrors, vec![official()]),
            Self::Official => (vec![official()], mirrors),
            Self::Bmcl | Self::HkmcMain | Self::HkmcEo => {
                let mut fallbacks: Vec<(Self, String)> = self
                    .mirror_bases()
                    .into_iter()
                    .filter_map(|base| {
                        let source = source_for_base(base);
                        (source != self).then(|| (source, manifest_url(source)))
                    })
                    .collect();
                fallbacks.push(official());
                (vec![(self, manifest_url(self))], fallbacks)
            }
        }
    }
}

fn manifest_url(source: DownloadSource) -> String {
    match source.base() {
        Some(base) => format!("{base}/mc/game/version_manifest.json"),
        None => format!("{OFFICIAL_META_BASE}/mc/game/version_manifest.json"),
    }
}

fn source_for_base(base: &str) -> DownloadSource {
    MIRROR_BASES
        .iter()
        .find_map(|(known, source)| (*known == base).then_some(*source))
        .unwrap_or(DownloadSource::Official)
}

/// 镜像 URL → 镜像内路径；不是已知镜像域名时返回 None
pub fn mirror_path(url: &str) -> Option<String> {
    MIRROR_BASES.iter().find_map(|(base, _)| {
        url.strip_prefix(base)
            .filter(|rest| rest.starts_with('/'))
            .map(str::to_string)
    })
}

/// 同一路径在各个镜像上的候选地址，首选源在最前
pub fn build_mirror_urls(source: DownloadSource, path: &str) -> Vec<String> {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    source
        .mirror_bases()
        .into_iter()
        .map(|base| format!("{base}{path}"))
        .collect()
}

/// 把版本清单里的 JSON 地址换成当前源
///
/// MC 光速清单本身就给出光速地址（主/EO 两个域名都识别），BMCL 清单给的是
/// piston-meta 官方地址，统一按路径替换；官方源保持原样。
pub fn version_json_url(url: &str, source: DownloadSource) -> String {
    let Some(base) = source.base() else {
        return url.to_string();
    };

    if let Some(path) = mirror_path(url) {
        return format!("{base}{path}");
    }

    url.replace("https://launchermeta.mojang.com", base)
        .replace("https://piston-meta.mojang.com", base)
}

pub fn client_jar_urls(
    source: DownloadSource,
    declared_url: Option<&str>,
    sha1: Option<&str>,
) -> Vec<String> {
    let Some(declared_url) = declared_url.filter(|url| !url.is_empty()) else {
        return Vec::new();
    };
    let sha1 = sha1.filter(|sha1| !sha1.trim().is_empty());

    if !source.is_mirror() {
        let mut urls = Vec::new();
        if mirror_path(declared_url).is_some() {
            // 本地 JSON 可能来自之前的镜像下载；官方模式下按 sha1 换回官方地址
            match sha1 {
                Some(sha1) => urls.push(format!(
                    "https://piston-data.mojang.com/v1/objects/{sha1}/client.jar"
                )),
                None => urls.push(declared_url.to_string()),
            }
        } else {
            urls.push(declared_url.to_string());
        }
        if let Some(sha1) = sha1 {
            for (base, _) in MIRROR_BASES {
                urls.push(format!("{base}/v1/objects/{sha1}/client.jar"));
            }
        }
        return dedup_urls(urls);
    }

    let mut urls = Vec::new();
    if let Some(path) = mirror_path(declared_url) {
        urls.extend(build_mirror_urls(source, &path));
    } else if let Some(sha1) = sha1 {
        // 官方地址只能按 sha1 拼镜像路径；两个镜像都支持 /v1/objects/{sha1}/{name}
        for base in source.mirror_bases() {
            urls.push(format!("{base}/v1/objects/{sha1}/client.jar"));
        }
    }
    urls.push(declared_url.to_string());
    dedup_urls(urls)
}

pub fn asset_index_urls(
    source: DownloadSource,
    declared_url: Option<&str>,
    sha1: Option<&str>,
    index_id: &str,
) -> Vec<String> {
    let declared_url = declared_url.filter(|url| !url.is_empty());
    let sha1 = sha1.filter(|sha1| !sha1.trim().is_empty());

    if !source.is_mirror() {
        let mut urls = Vec::new();
        match declared_url {
            Some(url) if mirror_path(url).is_none() => urls.push(url.to_string()),
            _ => {
                if let Some(sha1) = sha1 {
                    urls.push(format!(
                        "https://piston-meta.mojang.com/v1/packages/{sha1}/{index_id}.json"
                    ));
                } else if let Some(url) = declared_url {
                    urls.push(url.to_string());
                }
            }
        }
        if let Some(sha1) = sha1 {
            for (base, _) in MIRROR_BASES {
                urls.push(format!("{base}/v1/packages/{sha1}/{index_id}.json"));
            }
        }
        return dedup_urls(urls);
    }

    let mut urls = Vec::new();
    match declared_url.and_then(|url| mirror_path(url).map(|path| (url, path))) {
        Some((_, path)) => urls.extend(build_mirror_urls(source, &path)),
        None => {
            if let Some(sha1) = sha1 {
                for base in source.mirror_bases() {
                    urls.push(format!("{base}/v1/packages/{sha1}/{index_id}.json"));
                }
            }
        }
    }

    if let Some(url) = declared_url.filter(|url| mirror_path(url).is_none()) {
        urls.push(url.to_string());
    } else if let Some(sha1) = sha1 {
        urls.push(format!(
            "https://piston-meta.mojang.com/v1/packages/{sha1}/{index_id}.json"
        ));
    }

    dedup_urls(urls)
}

pub fn asset_object_urls(source: DownloadSource, prefix: &str, hash: &str) -> Vec<String> {
    if !source.is_mirror() {
        let mut urls = vec![format!("{OFFICIAL_RESOURCES_BASE}/{prefix}/{hash}")];
        for (base, _) in MIRROR_BASES {
            urls.push(format!("{base}/assets/{prefix}/{hash}"));
        }
        return dedup_urls(urls);
    }

    let mut urls: Vec<String> = source
        .mirror_bases()
        .into_iter()
        .map(|base| format!("{base}/assets/{prefix}/{hash}"))
        .collect();
    urls.push(format!("{OFFICIAL_RESOURCES_BASE}/{prefix}/{hash}"));
    dedup_urls(urls)
}

/// 库文件的候选链：镜像统一走 `/maven/{path}`（BMCL 与 MC 光速都支持），
/// 最后回退到版本 JSON 声明的官方地址，或按路径猜到的官方 maven。
pub fn library_urls(
    source: DownloadSource,
    relative_path: &str,
    declared_url: Option<&str>,
) -> Vec<String> {
    let path = relative_path.trim_start_matches('/');
    let declared_official = declared_url
        .filter(|url| !url.trim().is_empty())
        .filter(|url| mirror_path(url).is_none());

    if !source.is_mirror() {
        let mut urls = vec![
            declared_official
                .map(str::to_string)
                .unwrap_or_else(|| guess_official_maven(path)),
        ];
        for (base, _) in MIRROR_BASES {
            urls.push(format!("{base}/maven/{path}"));
        }
        return dedup_urls(urls);
    }

    let mut urls = build_mirror_urls(source, &format!("/maven/{path}"));
    urls.push(
        declared_official
            .map(str::to_string)
            .unwrap_or_else(|| guess_official_maven(path)),
    );
    dedup_urls(urls)
}

fn guess_official_maven(relative_path: &str) -> String {
    if relative_path.starts_with("net/minecraftforge/") {
        format!("{MAVEN_FORGE}/{relative_path}")
    } else if relative_path.starts_with("net/neoforged/") {
        format!("{MAVEN_NEOFORGE}/{relative_path}")
    } else {
        format!("{MAVEN_MINECRAFT}/{relative_path}")
    }
}

pub fn dedup_urls(urls: Vec<String>) -> Vec<String> {
    let mut unique = Vec::with_capacity(urls.len());
    for url in urls {
        if !url.is_empty() && !unique.contains(&url) {
            unique.push(url);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::{
        BMCL_BASE, DownloadSource, HKMC_EO_BASE, HKMC_MAIN_BASE, OFFICIAL_RESOURCES_BASE,
        asset_index_urls, asset_object_urls, build_mirror_urls, client_jar_urls, library_urls,
        version_json_url,
    };

    #[test]
    fn test_setting_mapping_uses_three_modes_and_keeps_legacy_values_as_mirror() {
        assert_eq!(DownloadSource::from_setting(None), DownloadSource::Auto);
        assert_eq!(DownloadSource::from_setting(Some("")), DownloadSource::Auto);
        assert_eq!(
            DownloadSource::from_setting(Some("auto")),
            DownloadSource::Auto
        );
        assert_eq!(
            DownloadSource::from_setting(Some("official")),
            DownloadSource::Official
        );
        assert_eq!(
            DownloadSource::from_setting(Some("mirror")),
            DownloadSource::Mirror
        );
        // 旧版本的三个镜像值都归到「优先镜像」
        assert_eq!(
            DownloadSource::from_setting(Some("bmcl")),
            DownloadSource::Mirror
        );
        assert_eq!(
            DownloadSource::from_setting(Some("hkmc")),
            DownloadSource::Mirror
        );
        assert_eq!(
            DownloadSource::from_setting(Some("hkmc-eo")),
            DownloadSource::Mirror
        );
        assert_eq!(
            DownloadSource::from_setting(Some("whatever")),
            DownloadSource::Mirror
        );
    }

    #[test]
    fn test_mirror_bases_prefer_selected_source_then_others() {
        assert_eq!(
            DownloadSource::HkmcMain.mirror_bases(),
            vec![HKMC_MAIN_BASE, HKMC_EO_BASE, BMCL_BASE]
        );
        assert_eq!(
            DownloadSource::HkmcEo.mirror_bases(),
            vec![HKMC_EO_BASE, HKMC_MAIN_BASE, BMCL_BASE]
        );
        assert_eq!(
            DownloadSource::Bmcl.mirror_bases(),
            vec![BMCL_BASE, HKMC_MAIN_BASE, HKMC_EO_BASE]
        );
        assert_eq!(
            DownloadSource::Mirror.mirror_bases(),
            vec![BMCL_BASE, HKMC_MAIN_BASE, HKMC_EO_BASE]
        );
        assert_eq!(
            DownloadSource::Auto.mirror_bases(),
            vec![BMCL_BASE, HKMC_MAIN_BASE, HKMC_EO_BASE]
        );
        assert!(DownloadSource::Official.mirror_bases().is_empty());
    }

    #[test]
    fn test_manifest_batches_match_the_three_modes() {
        // Auto：官方与镜像同一批竞速
        let (primary, fallback) = DownloadSource::Auto.manifest_batches();
        assert_eq!(primary.len(), 4);
        assert!(
            primary
                .iter()
                .any(|(source, _)| *source == DownloadSource::Official)
        );
        assert!(fallback.is_empty());

        // Mirror：镜像先竞速，官方只在全失败后兜底
        let (primary, fallback) = DownloadSource::Mirror.manifest_batches();
        assert_eq!(primary.len(), 3);
        assert!(
            primary
                .iter()
                .all(|(source, _)| *source != DownloadSource::Official)
        );
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].0, DownloadSource::Official);

        // Official：官方单独先试，镜像作为第二批
        let (primary, fallback) = DownloadSource::Official.manifest_batches();
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].0, DownloadSource::Official);
        assert_eq!(fallback.len(), 3);
    }

    #[test]
    fn test_version_json_url_rewrites_manifest_hosts() {
        // MC 光速清单里给的是光速域名，主线路要把 EO 域名换回来
        assert_eq!(
            version_json_url(
                "https://mirror.hkmc.online/v1/packages/abc/1.20.1.json",
                DownloadSource::HkmcMain
            ),
            "https://bsmirror.hkmc.online/v1/packages/abc/1.20.1.json"
        );
        // BMCL 清单给的是官方 piston 地址
        assert_eq!(
            version_json_url(
                "https://piston-meta.mojang.com/v1/packages/abc/1.20.1.json",
                DownloadSource::Bmcl
            ),
            "https://bmclapi2.bangbang93.com/v1/packages/abc/1.20.1.json"
        );
        // 官方源不动
        assert_eq!(
            version_json_url(
                "https://piston-meta.mojang.com/v1/packages/abc/1.20.1.json",
                DownloadSource::Official
            ),
            "https://piston-meta.mojang.com/v1/packages/abc/1.20.1.json"
        );
    }

    #[test]
    fn test_client_jar_urls_cover_both_mirrors_then_official() {
        let urls = client_jar_urls(
            DownloadSource::HkmcMain,
            Some("https://mirror.hkmc.online/v1/objects/abc/client.jar"),
            Some("abc"),
        );
        assert_eq!(
            urls[0],
            format!("{HKMC_MAIN_BASE}/v1/objects/abc/client.jar")
        );
        assert_eq!(urls[1], format!("{HKMC_EO_BASE}/v1/objects/abc/client.jar"));
        assert_eq!(urls[2], format!("{BMCL_BASE}/v1/objects/abc/client.jar"));

        // BMCL 清单给官方地址时按 sha1 拼镜像地址，官方地址留作最后兜底
        let urls = client_jar_urls(
            DownloadSource::Bmcl,
            Some("https://piston-data.mojang.com/v1/objects/abc/client.jar"),
            Some("abc"),
        );
        assert_eq!(urls[0], format!("{BMCL_BASE}/v1/objects/abc/client.jar"));
        assert!(
            urls.contains(&"https://piston-data.mojang.com/v1/objects/abc/client.jar".to_string())
        );

        // 官方模式：官方地址在最前，镜像作为备用
        let urls = client_jar_urls(
            DownloadSource::Official,
            Some("https://piston-data.mojang.com/v1/objects/abc/client.jar"),
            Some("abc"),
        );
        assert_eq!(
            urls[0],
            "https://piston-data.mojang.com/v1/objects/abc/client.jar"
        );
        assert_eq!(urls[1], format!("{BMCL_BASE}/v1/objects/abc/client.jar"));
    }

    #[test]
    fn test_asset_index_and_object_urls_include_official_fallback() {
        let urls = asset_index_urls(
            DownloadSource::HkmcEo,
            Some("https://mirror.hkmc.online/v1/packages/abc/5.json"),
            Some("abc"),
            "5",
        );
        assert_eq!(urls[0], format!("{HKMC_EO_BASE}/v1/packages/abc/5.json"));
        assert!(
            urls.contains(&"https://piston-meta.mojang.com/v1/packages/abc/5.json".to_string())
        );

        let urls = asset_object_urls(DownloadSource::HkmcMain, "01", "aabb");
        assert_eq!(urls[0], format!("{HKMC_MAIN_BASE}/assets/01/aabb"));
        assert_eq!(
            urls.last().unwrap(),
            &format!("{OFFICIAL_RESOURCES_BASE}/01/aabb")
        );

        // 官方模式下官方资源站在最前，镜像留作兜底
        let urls = asset_object_urls(DownloadSource::Official, "01", "aabb");
        assert_eq!(urls[0], format!("{OFFICIAL_RESOURCES_BASE}/01/aabb"));
        assert_eq!(urls[1], format!("{BMCL_BASE}/assets/01/aabb"));
    }

    #[test]
    fn test_library_urls_use_universal_maven_path_and_official_guess() {
        let urls = library_urls(
            DownloadSource::HkmcMain,
            "com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
            Some(
                "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
            ),
        );
        assert_eq!(
            urls[0],
            format!("{HKMC_MAIN_BASE}/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar")
        );
        assert_eq!(
            urls.last().unwrap(),
            "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar"
        );

        // 声明地址本身是镜像时（从光速 JSON 解析出来），官方兜底按路径猜
        let urls = library_urls(
            DownloadSource::HkmcMain,
            "net/neoforged/fancymodloader/loader/1.0/loader-1.0.jar",
            Some(
                "https://mirror.hkmc.online/maven/net/neoforged/fancymodloader/loader/1.0/loader-1.0.jar",
            ),
        );
        assert_eq!(
            urls.last().unwrap(),
            "https://maven.neoforged.net/releases/net/neoforged/fancymodloader/loader/1.0/loader-1.0.jar"
        );

        // 官方模式：官方 maven 在最前，镜像作为备用
        let urls = library_urls(
            DownloadSource::Official,
            "com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
            Some(
                "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
            ),
        );
        assert_eq!(
            urls[0],
            "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar"
        );
        assert_eq!(
            urls[1],
            format!("{BMCL_BASE}/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar")
        );
    }

    #[test]
    fn test_build_mirror_urls_handles_leading_slash() {
        assert_eq!(
            build_mirror_urls(DownloadSource::Bmcl, "maven/x.jar"),
            vec![
                format!("{BMCL_BASE}/maven/x.jar"),
                format!("{HKMC_MAIN_BASE}/maven/x.jar"),
                format!("{HKMC_EO_BASE}/maven/x.jar"),
            ]
        );
    }
}
