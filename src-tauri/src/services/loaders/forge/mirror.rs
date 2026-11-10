//! 下载镜像：官方源优先，BMCLAPI 兜底

use super::{MAVEN_CENTRAL, MAVEN_FORGE, MAVEN_MINECRAFT};
use crate::services::download::source::{self as download_sources, DownloadSource};

/// 官方 maven 地址 → 镜像内路径
///
/// 只映射镜像确实覆盖了的域名；镜像统一用 `/maven/{path}`（BMCL 与 MC 光速都支持），
/// 不区分 Mojang 库与 Forge/NeoForge 库。
pub(crate) fn maven_mirror_path(url: &str) -> Option<String> {
    let path = [
        MAVEN_MINECRAFT,
        MAVEN_FORGE,
        "https://maven.neoforged.net/releases",
        MAVEN_CENTRAL,
    ]
    .iter()
    .find_map(|official| {
        url.strip_prefix(official)
            .map(|rest| rest.trim_start_matches('/').to_string())
    })?;

    Some(format!("/maven/{path}"))
}

/// 官方 maven 地址在当前下载源下的候选镜像地址
pub(crate) fn mirror_urls(url: &str, source: DownloadSource) -> Vec<String> {
    if let Some(path) = download_sources::mirror_path(url) {
        return download_sources::build_mirror_urls(source, &path);
    }

    maven_mirror_path(url)
        .map(|path| download_sources::build_mirror_urls(source, &path))
        .unwrap_or_default()
}

/// Forge 安装器的候选下载地址。
/// maven 坐标在 1.8 与 1.9 之间换过形式（≤1.8 带 -{mc} 后缀，≥1.9 不带），
/// 所以两种形式都进候选表，版本判断只决定先试哪个；只靠判断挑一种，漏掉的版本就永远装不上
pub(crate) fn forge_installer_sources(mc: &str, v: &str) -> Vec<String> {
    let suffixed = format!("{mc}-{v}-{mc}");
    let plain = format!("{mc}-{v}");

    let prefer_suffixed = crate::utils::minecraft::parse_mc_version(mc)
        .is_some_and(|(major, minor)| major == 1 && minor <= 8);

    let coords = if prefer_suffixed {
        vec![suffixed, plain]
    } else {
        vec![plain, suffixed]
    };

    let mut sources = Vec::with_capacity(coords.len() * 4);
    let source = DownloadSource::current();

    for coord in &coords {
        let path = format!("net/minecraftforge/forge/{coord}/forge-{coord}-installer.jar");
        sources.extend(download_sources::build_mirror_urls(
            source,
            &format!("/maven/{path}"),
        ));
        sources.push(format!("{MAVEN_FORGE}/{path}"));
    }

    download_sources::dedup_urls(sources)
}
