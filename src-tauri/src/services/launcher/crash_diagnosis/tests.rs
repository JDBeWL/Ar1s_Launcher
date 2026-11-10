use super::*;

/// 取自本机 2026-10-17 18:40 用 Java 26 启动 Closing Song1.6.6.1 的真实 stderr
const MIXIN_CRASH: &str = r#"[18:40:23] [main/FATAL] [mixin/]: Mixin apply for mod fancymenu failed fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> net.minecraftforge.client.loading.ForgeLoadingOverlay: org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException Unexpecteded ClassMetadataNotFoundException whilst transforming the mixin class: [MAIN Applicator Phase -> fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> Apply Methods -> (Lnet/minecraft/client/gui/screens/Screen;Lnet/minecraft/client/Minecraft;IILcom/llamalad7/mixinextras/injector/wrapoperation/Operation;)V:wrapOperation$ddg001$fancymenu$wrapInitScreenFancyMenu -> Transform Descriptor -> desc=Lnet/minecraft/client/Minecraft;]
org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException: Unexpecteded ClassMetadataNotFoundException whilst transforming the mixin class: [MAIN Applicator Phase -> fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> Transform Descriptor -> desc=Lnet/minecraft/client/Minecraft;]
	at org.spongepowered.asm.mixin.transformer.MixinTargetContext.transformMethod(MixinTargetContext.java:510) ~[mixin-0.8.5.jar:0.8.5+Jenkins-b310]
	at cpw.mods.modlauncher.Launcher.run(Launcher.java:108) ~[modlauncher-10.0.9.jar:?]
Caused by: org.spongepowered.asm.mixin.throwables.ClassMetadataNotFoundException: net.minecraft.client.Minecraft
	at org.spongepowered.asm.mixin.transformer.MixinTargetContext.transformSingleDescriptor(MixinTargetContext.java:1001) ~[mixin-0.8.5.jar:0.8.5+Jenkins-b310]
	... 35 more
Exception in thread "main" java.lang.RuntimeException: org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered
	at cpw.mods.modlauncher.LaunchServiceHandlerDecorator.launch(LaunchServiceHandlerDecorator.java:32)
Caused by: org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered
	at org.spongepowered.asm.mixin.transformer.MixinProcessor.applyMixins(MixinProcessor.java:392)
Caused by: org.spongepowered.asm.mixin.throwables.MixinApplyError: Mixin [fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu] from phase [DEFAULT] in config [fancymenu.forge.mixins.json] FAILED during APPLY
	at org.spongepowered.asm.mixin.transformer.MixinProcessor.handleMixinError(MixinProcessor.java:638)
Caused by: org.spongepowered.asm.mixin.throwables.ClassMetadataNotFoundException: net.minecraft.client.Minecraft
	at org.spongepowered.asm.mixin.transformer.MixinTargetContext.transformSingleDescriptor(MixinTargetContext.java:1001)"#;

fn lines(text: &str) -> Vec<String> {
    text.lines().map(|l| l.to_string()).collect()
}

fn ctx_java_26() -> LaunchContext {
    LaunchContext {
        version_id: "Closing Song1.6.6.1".to_string(),
        loader: "Forge".to_string(),
        declared_java: Some(17),
        used_java_path: "C:/Program Files/Zulu/zulu-26/bin/java.exe".to_string(),
        used_java_version: Some(26),
        memory_mb: 9600,
        mod_files: vec![
            "Connector-1.0.0-beta.46+1.20.1.jar".to_string(),
            "fancymenu_forge_3.3.2_MC_1.20.1.jar".to_string(),
            "Jade-1.20.1-Forge-11.12.2.jar".to_string(),
        ],
        runtime_secs: None,
    }
}

#[test]
fn test_mixin_failure_names_the_mod_behind_the_crash() {
    let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

    assert!(d.recognized);
    let mixin = d
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::Mixin)
        .expect("应当识别出 mixin 失败");
    assert_eq!(mixin.confidence, Confidence::High);
    assert_eq!(mixin.culprits, vec!["fancymenu_forge_3.3.2_MC_1.20.1.jar"]);
    assert_eq!(mixin.culprit_label, "问题模组");
    assert_eq!(mixin.evidence[0].line_no, 1);
    assert!(d.headline.contains("fancymenu"));
    // 配置名是证据的一部分，但不能混进「去更新哪个模组」的建议里
    assert!(mixin.conclusion.contains("fancymenu.forge.mixins.json"));
    assert!(!mixin.suggestion.contains("mixins.json"));
}

#[test]
fn test_mixin_failure_with_newer_java_than_declared_flags_java() {
    let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

    let java = d
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::JavaVersion)
        .expect("本次 Java 高于声明时应当把 Java 列为嫌疑");
    assert_eq!(java.confidence, Confidence::Medium);
    assert!(java.conclusion.contains("26"));
    assert!(java.conclusion.contains("17"));
}

fn ctx_plain() -> LaunchContext {
    LaunchContext {
        version_id: "test".to_string(),
        loader: "Forge".to_string(),
        declared_java: Some(17),
        used_java_path: "C:/java-17/bin/java.exe".to_string(),
        used_java_version: Some(17),
        memory_mb: 4096,
        mod_files: Vec::new(),
        runtime_secs: None,
    }
}

fn only(d: &CrashDiagnosis, kind: FindingKind) -> Finding {
    d.findings
        .iter()
        .find(|f| f.kind == kind)
        .unwrap_or_else(|| panic!("期望有 {:?}，实际 {:?}", kind, d.findings))
        .clone()
}

#[test]
fn test_unsupported_class_version_points_at_the_required_java() {
    let log = "Exception in thread \"main\" java.lang.UnsupportedClassVersionError: dev/kaetus/mymod/Main has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0\n\tat java.base/java.lang.ClassLoader.defineClass1(Native Method)";

    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    let f = only(&d, FindingKind::ClassVersion);
    assert_eq!(f.confidence, Confidence::High);
    assert_eq!(f.culprits, vec!["dev/kaetus/mymod/Main"]);
    assert_eq!(f.culprit_label, "问题类");
    assert!(f.conclusion.contains("65"));
    assert!(f.suggestion.contains("21"), "应当把 65 折成所需 Java");
}

#[test]
fn test_missing_class_is_reported_as_absent_dependency() {
    let log = "java.lang.NoClassDefFoundError: me/shedaniel/autoconfig/ui/ConfigListScreen\n\tat net.minecraftforge.client.ForgeHooksClient.openScreen(ForgeHooksClient.java:120)\nCaused by: java.lang.ClassNotFoundException: me.shedaniel.autoconfig.ui.ConfigListScreen\n\tat java.base/java.net.URLClassLoader.findClass(URLClassLoader.java:445)";

    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    let f = only(&d, FindingKind::MissingClass);
    assert_eq!(f.confidence, Confidence::Medium);
    assert_eq!(
        f.culprits,
        vec!["me/shedaniel/autoconfig/ui/ConfigListScreen"]
    );
    assert!(f.suggestion.contains("前置"));
    assert_eq!(f.culprit_label, "问题类");
    // 结论只说事实；被降级成表象时不能还留着「前置模组没装」这种指控
    assert!(!f.conclusion.contains("前置"));
}

#[test]
fn test_out_of_memory_quotes_the_memory_we_configured() {
    let log = "[18:52:01] [Render thread/ERROR]: Unhandled exception\njava.lang.OutOfMemoryError: Java heap space\n\tat net.minecraft.world.chunk.ChunkArray.<init>(ChunkArray.java:22)";

    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    let f = only(&d, FindingKind::Memory);
    assert_eq!(f.confidence, Confidence::High);
    assert!(f.conclusion.contains("4096"), "要说清本次给了多少堆");
}

#[test]
fn test_unsatisfied_link_error_points_at_natives() {
    let log = "Caused by: java.lang.UnsatisfiedLinkError: Could not load native library: lwjgl.dll, OS: Windows 11\n\tat org.lwjgl.system.libc.Libc.<clinit>(Libc.java:40)";

    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    let f = only(&d, FindingKind::Natives);
    assert!(f.culprits.iter().any(|c| c.contains("lwjgl.dll")));
    assert_eq!(f.culprit_label, "问题文件");
    assert!(f.suggestion.contains("natives"));
}

#[test]
fn test_security_manager_failure_means_java_is_too_new() {
    let log = "java.lang.UnsupportedOperationException: The Security Manager is deprecated and will be removed\n\tat java.base/java.lang.System.setSecurityManager(System.java:403)\n\tat net.minecraftforge.fml.loading.FMLLoader.setupSecurity(FMLLoader.java:96)";

    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    let f = only(&d, FindingKind::SecurityManager);
    assert_eq!(f.confidence, Confidence::High);
    assert!(f.evidence[0].text.contains("Security Manager"));
    assert_eq!(f.culprit_label, "本次用的 Java");
}

/// 取自 19:42 那次用 Java 26 启动同一整合包的 stderr 原文
const ASM_CRASH: &str = r#"[19:42:16] [main/WARN] [mixin/]: Error loading class: net/minecraft/server/level/ServerLevel (java.lang.IllegalArgumentException: Unsupported class file major version 70)
[19:42:17] [main/WARN] [mixin/]: Error loading class: com/simibubi/create/content/fluids/spout/FillingBySpout (java.lang.ClassNotFoundException: com.simibubi.create.content.fluids.spout.FillingBySpout)
[19:42:17] [main/WARN] [mixin/]: Error loading class: net/minecraft/client/Minecraft (java.lang.IllegalArgumentException: Unsupported class file major version 70)
[19:42:19] [main/FATAL] [mixin/]: Mixin apply for mod fancymenu failed fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> net.minecraftforge.client.loading.ForgeLoadingOverlay: org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException Unexpecteded ClassMetadataNotFoundException whilst transforming the mixin class
Caused by: org.spongepowered.asm.mixin.throwables.ClassMetadataNotFoundException: net.minecraft.client.Minecraft"#;

#[test]
fn test_asm_cannot_read_this_jdk_becomes_the_root_cause() {
    let d = diagnose(Some(1), &lines(ASM_CRASH), &ctx_java_26(), None);

    let f = only(&d, FindingKind::ClassReaderTooOld);
    assert_eq!(f.role, FindingRole::Root);
    assert_eq!(f.confidence, Confidence::High);
    assert!(f.conclusion.contains("major 70"));
    assert!(f.conclusion.contains("Java 26"));
    assert!(f.conclusion.contains("2 次"), "该说清这类报错有多少");
    assert_eq!(f.culprit_label, "本次用的 Java");
    assert!(f.suggestion.contains("17"));
}

#[test]
fn test_downstream_mixin_becomes_symptom() {
    let d = diagnose(Some(1), &lines(ASM_CRASH), &ctx_java_26(), None);

    assert_eq!(
        d.findings.first().unwrap().kind,
        FindingKind::ClassReaderTooOld
    );
    let mixin = only(&d, FindingKind::Mixin);
    assert_eq!(mixin.role, FindingRole::Symptom);
    assert!(mixin.suggestion.contains("根因"));
    assert!(!mixin.suggestion.contains("移出 mods"));
    assert!(
        !d.findings
            .iter()
            .any(|f| f.kind == FindingKind::MissingClass),
        "mixin 探测可选目标的 WARN 不能当成缺类证据"
    );
    assert!(
        !d.findings
            .iter()
            .any(|f| f.kind == FindingKind::JavaVersion),
        "根因已经点明 Java，不该再补一条同样的推断"
    );
}

/// 取自本机 2026-10-18 22:34 正常启动的 Vanilla Perfected 26.3 日志：
/// mixin 探测 Sodium 的几个可选目标失败，游戏照常跑完
const MIXIN_PROBE_WARNS: &str = r#"[22:34:41] [main/WARN]: Error loading class: me/jellysquid/mods/sodium/client/render/chunk/DefaultChunkRenderer (java.lang.ClassNotFoundException: me/jellysquid/mods/sodium/client/render/chunk/DefaultChunkRenderer)
[22:34:42] [main/WARN]: Error loading class: me/jellysquid/mods/sodium/client/render/chunk/RenderSectionManager (java.lang.ClassNotFoundException: me/jellysquid/mods/sodium/client/render/chunk/RenderSectionManager)
[22:34:42] [main/WARN]: Error loading class: me/jellysquid/mods/sodium/client/render/SodiumWorldRenderer (java.lang.ClassNotFoundException: me/jellysquid/mods/sodium/client/render/SodiumWorldRenderer)"#;

#[test]
fn test_mixin_probe_warns_are_not_a_missing_class_root_cause() {
    let d = diagnose(Some(-8), &lines(MIXIN_PROBE_WARNS), &ctx_plain(), None);

    assert!(
        !d.findings
            .iter()
            .any(|f| f.kind == FindingKind::MissingClass),
        "这些 WARN 在正常跑完的游戏里也会出现，不能当缺类根因：{:?}",
        d.findings
    );
}

#[test]
fn test_headline_is_the_short_summary_not_the_full_conclusion() {
    let d = diagnose(Some(1), &lines(ASM_CRASH), &ctx_java_26(), None);

    assert_eq!(d.headline, d.findings[0].summary);
    assert!(!d.headline.contains("net.minecraftforge.client.loading"));
    let mixin = only(&d, FindingKind::Mixin);
    assert!(mixin.summary.contains("fancymenu"));
    assert!(mixin.summary.chars().count() < 40);
}

#[test]
fn test_mixin_alone_is_the_root_cause() {
    let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

    let mixin = only(&d, FindingKind::Mixin);
    assert_eq!(mixin.role, FindingRole::Root);
    assert!(mixin.suggestion.contains("移出 mods"));
    assert!(
        !d.headline.contains("ClassMetadataNotFoundException"),
        "顶部不能再抄一遍完整结论"
    );
}

#[test]
fn test_unrecognized_crash_admits_it_and_keeps_the_cause_chain() {
    let log = "[18:52:01] [main/ERROR]: Something went wrong\njava.lang.IllegalStateException: registry blowup\n\tat com.example.Thing.marshal(Thing.java:9)\nCaused by: com.example.NestedException: 缺少必需的配置项 registry_seed\n\tat com.example.Thing.<init>(Thing.java:4)";

    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    assert!(!d.recognized, "没有特征命中时不能装作认出来了");
    assert!(d.headline.contains("未识别"));
    let unknown = only(&d, FindingKind::Unknown);
    assert!(unknown.culprits.is_empty());
    assert!(
        unknown
            .conclusion
            .contains("缺少必需的配置项 registry_seed")
    );
}

/// 游戏自己写下的报告：stdout 没有可用特征时，报告里的阶段/异常/直接原因才是证据
const GAME_CRASH_REPORT: &str = r#"---- Minecraft Crash Report ----
// I bet Cylons wouldn't have this problem.
Time: 2026-10-18 22:35:26
Description: Rendering overlay

java.lang.NullPointerException: Cannot invoke "net.minecraft.client.Minecraft" because "this.mc" is null
	at com.example.Mod.overlay(Mod.java:10)
Caused by: java.lang.IllegalStateException: renderer already closed
	at com.example.Renderer.close(Renderer.java:42)

A detailed walkthrough of the error, its code path and all known details is as follows:
"#;

#[test]
fn test_game_crash_report_supplies_a_finding_when_stdout_has_none() {
    let d = diagnose_with_crash_report(
        Some(1),
        &[],
        &ctx_plain(),
        None,
        Some(CrashReportEvidence {
            path: "crash-reports/crash-test-client.txt",
            text: GAME_CRASH_REPORT,
        }),
    );

    assert!(d.recognized);
    let f = only(&d, FindingKind::GameCrashReport);
    assert_eq!(f.role, FindingRole::Root);
    assert!(f.summary.contains("Rendering overlay"));
    assert!(f.conclusion.contains("NullPointerException"));
    assert!(f.conclusion.contains("renderer already closed"));
    assert_eq!(f.evidence.len(), 3);
    assert_eq!(f.evidence[0].line_no, 4);
    assert!(
        d.context
            .iter()
            .any(|c| c.label == "崩溃阶段" && c.value == "Rendering overlay")
    );
    assert!(
        d.context
            .iter()
            .any(|c| c.label == "崩溃报告" && c.value.ends_with("crash-test-client.txt"))
    );
    assert!(d.raw_output.contains("--- 游戏崩溃报告"));
    assert!(d.report_text.contains("renderer already closed"));
}

#[test]
fn test_existing_stdout_root_keeps_precedence_over_the_report() {
    let log = "java.lang.OutOfMemoryError: Java heap space";

    let d = diagnose_with_crash_report(
        Some(1),
        &lines(log),
        &ctx_plain(),
        None,
        Some(CrashReportEvidence {
            path: "crash-reports/crash-test-client.txt",
            text: GAME_CRASH_REPORT,
        }),
    );

    only(&d, FindingKind::Memory);
    assert!(
        !d.findings
            .iter()
            .any(|f| f.kind == FindingKind::GameCrashReport),
        "stdout 已经定位到根因时，报告摘要不该抢 headline"
    );
    assert!(
        d.context.iter().any(|c| c.label == "崩溃报告"),
        "报告路径仍要附在环境里"
    );
}

#[test]
fn test_report_evidence_does_not_shift_stdout_excerpt() {
    let stdout = lines("stdout first line\nstdout second line");

    let d = diagnose_with_crash_report(
        Some(1),
        &stdout,
        &ctx_plain(),
        None,
        Some(CrashReportEvidence {
            path: "crash-reports/crash-test-client.txt",
            text: GAME_CRASH_REPORT,
        }),
    );

    assert!(
        d.excerpt.iter().all(|e| e.text.starts_with("stdout")),
        "报告行号不能拿来截 stdout: {:?}",
        d.excerpt
    );
}

#[test]
fn test_hs_err_report_becomes_a_jvm_crash_finding() {
    let hs = "# A fatal error has been detected by the Java Runtime Environment:\n#\t# Internal Error (safepoint.cpp:512), pid=1234, tid=5678\n#\t# Problematic frame:\n#\t# V  [libjvm.dll+0x8a1b2c]";

    let d = diagnose(Some(1), &[], &ctx_plain(), Some(hs));

    assert!(d.recognized);
    let f = only(&d, FindingKind::JvmCrash);
    assert!(f.culprits.iter().any(|c| c.contains("libjvm.dll")));
    assert_eq!(f.culprit_label, "问题文件");
    assert_eq!(d.findings.first().unwrap().kind, FindingKind::JvmCrash);
}

#[test]
fn test_context_lines_explain_the_environment() {
    let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

    let value = |label: &str| {
        d.context
            .iter()
            .find(|c| c.label == label)
            .map(|c| c.value.clone())
    };
    assert_eq!(value("实例").as_deref(), Some("Closing Song1.6.6.1"));
    assert!(value("本次 Java").unwrap().contains("26"));
    assert!(value("本次 Java").unwrap().contains("zulu-26"));
    assert_eq!(value("模组数").as_deref(), Some("3"));
}

#[test]
fn test_excerpt_keeps_the_window_around_the_hit_line() {
    let mut big: Vec<String> = (1..=150).map(|i| format!("前置噪音第 {} 行", i)).collect();
    big.extend(lines(MIXIN_CRASH));

    let d = diagnose(Some(1), &big, &ctx_java_26(), None);

    assert!(
        d.excerpt
            .iter()
            .any(|e| e.text.contains("Mixin apply for mod fancymenu"))
    );
    assert!(d.excerpt[0].line_no > 130, "窗口应当跟着命中行走");
    assert_eq!(d.raw_output.lines().count(), big.len(), "原文不能只留尾巴");
}

#[test]
fn test_raw_output_trims_the_head_and_says_how_much() {
    let big: Vec<String> = (1..=2500).map(|i| format!("line {}", i)).collect();

    let d = diagnose(Some(1), &big, &ctx_plain(), None);

    assert_eq!(d.raw_output.lines().count(), RAW_OUTPUT_MAX_LINES + 1);
    assert!(d.raw_output.starts_with("… 前面 500 行未收录"));
    assert!(d.raw_output.ends_with("line 2500"));
}

#[test]
fn test_report_text_carries_conclusion_evidence_and_environment() {
    let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

    assert!(d.report_text.contains(&d.headline));
    assert!(d.report_text.contains("根因 · 直接证据"));
    assert!(d.report_text.contains("net.minecraft.client.Minecraft"));
    assert!(d.report_text.contains("本次 Java"));
    assert!(
        d.report_text
            .contains("问题模组：fancymenu_forge_3.3.2_MC_1.20.1.jar")
    );
}

/// Java 匹配由启动闸门负责，崩溃报告里再列一遍「声明 Java」会把人引偏
#[test]
fn test_context_reports_runtime_and_loader_not_the_java_gate_again() {
    let mut ctx = ctx_java_26();
    ctx.runtime_secs = Some(95);
    let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx, None);

    let labels: Vec<&str> = d.context.iter().map(|c| c.label.as_str()).collect();
    assert!(!labels.contains(&"声明 Java"));
    assert!(labels.contains(&"加载器"));
    assert!(labels.contains(&"启动到崩溃"));
    assert!(
        d.context
            .iter()
            .any(|c| c.label == "启动到崩溃" && c.value.contains("95"))
    );
}

/// Java 与声明一致时不该把它当成嫌疑，否则每条崩溃都带上同一句无关提示
#[test]
fn test_java_rule_stays_quiet_when_used_matches_declared() {
    let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_plain(), None);

    assert!(
        !d.findings
            .iter()
            .any(|f| f.kind == FindingKind::JavaVersion)
    );
}

/// 1.13~1.16 的 Forge 撞上 Java 9+ 的模块系统时，日志里长这样
const MODULE_ACCESS_CRASH: &str = r#"Exception in thread "main" java.lang.reflect.InvocationTargetException
	at java.base/jdk.internal.reflect.NativeMethodAccessorImpl.invoke0(Native Method)
	at cpw.mods.modlauncher.Launcher.main(Launcher.java:62)
Caused by: java.lang.reflect.InaccessibleObjectException: Unable to make field private final java.lang.String java.lang.Class.name accessible: module java.base does not "opens java.lang" to unnamed module @3d4eac69
	at java.base/java.lang.reflect.AccessibleObject.checkCanSetAccessible(AccessibleObject.java:354)"#;

fn ctx_java_25_declared_8() -> LaunchContext {
    LaunchContext {
        version_id: "1.13.2-Forge".to_string(),
        loader: "Forge".to_string(),
        declared_java: Some(8),
        used_java_path: "D:/Program Files/BellSoft/LibericaJDK-25/bin/java.exe".to_string(),
        used_java_version: Some(25),
        memory_mb: 9600,
        mod_files: Vec::new(),
        runtime_secs: Some(1),
    }
}

#[test]
fn test_module_access_with_newer_java_is_a_high_confidence_root_cause() {
    let d = diagnose(
        Some(1),
        &lines(MODULE_ACCESS_CRASH),
        &ctx_java_25_declared_8(),
        None,
    );

    assert!(d.recognized, "模块访问被拒应当算已识别");
    let java = d
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::JavaVersion)
        .expect("应当把 Java 版本列为根因");
    assert_eq!(java.confidence, Confidence::High);
    assert_eq!(java.role, FindingRole::Root);
    assert!(java.conclusion.contains("25"));
    assert!(java.conclusion.contains("8"));
    assert!(java.suggestion.contains("Java 8"), "{}", java.suggestion);

    // 已经点名 Java 之后不该再补一条更弱的同义推断
    assert_eq!(
        d.findings
            .iter()
            .filter(|f| f.kind == FindingKind::JavaVersion)
            .count(),
        1
    );
}

#[test]
fn test_module_access_is_not_blamed_on_java_when_versions_match() {
    let mut ctx = ctx_java_25_declared_8();
    ctx.declared_java = Some(25);
    let d = diagnose(Some(1), &lines(MODULE_ACCESS_CRASH), &ctx, None);

    // 声明与本次一致，那是模组自己要 add-opens，不是 JDK 太新
    assert!(
        !d.findings
            .iter()
            .any(|f| f.kind == FindingKind::JavaVersion)
    );
}

#[test]
fn test_unknown_rule_prefers_the_exception_over_a_stack_frame() {
    // 尾巴只剩栈帧时，把 at cpw.mods.modlauncher... 当根因没有意义
    let text = "Exception in thread \"main\" java.lang.reflect.InvocationTargetException\n\tat cpw.mods.modlauncher.Launcher.main(Launcher.java:62)";
    let d = diagnose(Some(1), &lines(text), &ctx_java_25_declared_8(), None);

    let unknown = d
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::Unknown)
        .expect("没有特征命中时应当给出未识别");
    assert!(
        unknown.conclusion.contains("InvocationTargetException"),
        "{}",
        unknown.conclusion
    );
    assert!(!unknown.conclusion.contains("最深的异常原因：at "));
}

#[test]
fn test_unknown_rule_does_not_call_a_frame_a_root_cause() {
    let text = "\tat cpw.mods.modlauncher.Launcher.main(Launcher.java:62)";
    let d = diagnose(Some(1), &lines(text), &ctx_java_25_declared_8(), None);

    let unknown = d
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::Unknown)
        .expect("应当给出未识别");
    assert!(
        unknown.conclusion.contains("没有异常类型"),
        "{}",
        unknown.conclusion
    );
}

/// 参数被拆开后，JVM 会把某个 `-D` 的值当成主类；
/// 这条规则要能说清“不是缺文件，是参数拼坏了”
#[test]
fn test_orphan_jvm_value_is_reported_as_broken_arguments() {
    let log = "Error: Could not find or load main class java.base/jdk.internal.misc=ALL-UNNAMED
Caused by: java.lang.ClassNotFoundException: java.base/jdk.internal.misc=ALL-UNNAMED";
    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    let f = only(&d, FindingKind::MainClass);
    assert_eq!(f.role, FindingRole::Root);
    assert_eq!(f.confidence, Confidence::High);
    assert!(f.conclusion.contains("参数"), "{}", f.conclusion);
    assert!(f.suggestion.contains("重建"), "{}", f.suggestion);
}

#[test]
fn test_localized_main_class_error_is_recognized() {
    let log = "错误: 找不到或无法加载主类 net.neoforged.fml.startup.Client
原因: java.lang.ClassNotFoundException: net.neoforged.fml.startup.Client";
    let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

    let f = only(&d, FindingKind::MainClass);
    assert!(f.conclusion.contains("classpath"), "{}", f.conclusion);
    assert_eq!(
        f.culprits,
        vec!["net.neoforged.fml.startup.Client".to_string()]
    );
}

fn ctx_quilt_26_3() -> LaunchContext {
    LaunchContext {
        version_id: "26.3-Quilt".to_string(),
        loader: "Quilt".to_string(),
        declared_java: Some(25),
        used_java_path: "D:/Program Files/BellSoft/LibericaJDK-25/bin/java.exe".to_string(),
        used_java_version: Some(25),
        memory_mb: 9600,
        mod_files: Vec::new(),
        runtime_secs: Some(1),
    }
}

/// Quilt 0.20.0-beta.9 自带 ASM 9.5，读不了 Java 25 的类。
/// 此时“换 Java”是句废话——本次就是 25，该换的是加载器。
#[test]
fn test_loader_too_old_advice_is_to_upgrade_loader_not_java() {
    let log = "\
[01:50:42] [main/INFO]: Loading Minecraft 26.3 with Quilt Loader 0.20.0-beta.9
[01:50:42] [main/INFO]: Mappings not present!
[01:50:42] [main/ERROR]: Uncaught exception in thread \"main\"
java.lang.IllegalArgumentException: Unsupported class file major version 69
\tat org.objectweb.asm.ClassReader.<init>(ClassReader.java:199)
\tat org.quiltmc.loader.impl.entrypoint.GameTransformer.lambda$locateEntrypoints$0(GameTransformer.java:89)
\tat org.quiltmc.loader.impl.game.minecraft.MinecraftGameProvider.initialize(MinecraftGameProvider.java:407)";

    let d = diagnose(Some(1), &lines(log), &ctx_quilt_26_3(), None);
    let finding = only(&d, FindingKind::ClassReaderTooOld);

    assert!(
        finding.suggestion.contains("升级"),
        "应指向升级加载器: {}",
        finding.suggestion
    );
    assert!(
        finding.suggestion.contains("Quilt"),
        "应点名是哪个加载器: {}",
        finding.suggestion
    );
    assert!(
        !finding.suggestion.contains("换成版本 JSON 声明的 Java 25"),
        "本次已经是 Java 25，再说换 Java 是误导: {}",
        finding.suggestion
    );
    assert_eq!(finding.role, FindingRole::Root);
}

#[test]
fn test_rule_table_keeps_its_priority_order() {
    // 顺序即优先级：findings[0] 会变成弹窗标题，兜底规则永远排在最后
    assert_eq!(
        super::rules::rule_names(),
        vec![
            "jvm_crash",
            "class_reader_too_old",
            "mixin",
            "class_version",
            "main_class",
            "missing_class",
            "memory",
            "natives",
            "security_manager",
            "module_access",
            "game_crash_report",
            "unknown",
        ]
    );
}
