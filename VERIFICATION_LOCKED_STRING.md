0.4.10-locked-string - 锁定琴弦时启动收音，修复换弦读数和状态。

# 锁定弦修复验证

基线是 fix/tuner-audio-capture-recovery 的 5ddf418e76473d77cd7e5b1b02d3520a5a7325f1，即 0.4.9。本次包含 PR #2 的锁定弦修复，并保留基线中的录音恢复和启动绘制修复。正式版本升级为 0.4.10-locked-string，版本代码 14，沿用原正式签名。

## 源码中确认的路径

`poll_controls` 和 `process_pending_touch` 原来都只调用 `UiState::set_target`。这会改变锁定目标，但不会设置 `listening_requested`、申请麦克风权限或启动录音。同时 `set_target` 无论当前是否录音都提示“轻拨这根空弦”，导致用户直接锁定后拨弦时看不到任何读数。

换弦时原方法清空音高数据，却保留 Reading/Fading 阶段；权限、停止或错误提示也会被统一的拨弦提示覆盖。本补丁修正这些状态不一致。上述是源码可确认的故障路径，不等于已经通过用户手机日志确认唯一原因。

## 本次改动

两条选弦入口统一进入 `handle_target_selection`，先由可测试的 `selection::select_target` 更新目标，再在释放 UI 互斥锁后处理 StartListening 意图。

前台从未录音状态锁定一根有效琴弦，会复用原开始按钮的权限、重试和单录音线程流程；没有权限仍必须正常授权。录音中换弦只更改目标，不调用开始/停止切换操作，不另开录音器。权限请求未结束时不会重复请求。取消锁定只恢复自动选弦，不启动已停止的录音，也不停止正在进行的录音。

换弦后清除旧频率、指针、上一目标和调准确认帧，回到等待新声音阶段。重复目标事件不重置读数，非法索引不改变目标。等待声音时明确提示当前锁定弦；未开始、权限不足、暂停或麦克风错误时保留对应提示，不把“已锁定”当成“正在收音”。选弦操作写入 XianZhun 日志，不记录音频。

界面布局、音高分析及误识别阈值、六弦标准频率和权限范围保持既有设置。正式 APK 作为 GitHub Release 附件分发，旧 APK 在本地 old_apks 归档。

## 回归测试与执行边界

新增 `rust/src/selection.rs` 中 13 个测试，覆盖六弦从空闲状态锁定后的启动意图、停止/错误后重试、权限与前后台组合、录音中换弦、旧读数清除、重复/非法事件、取消锁定、状态提示、音分方向和错弦不得显示调准。

其中一个测试实际调用生产 PitchAnalyzer 与 UiState，以合成 PCM 验证六根弦在 44.1 kHz / 48 kHz 下连续三帧形成锁定读数。该测试不使用真实麦克风，不能替代 JNI 调用和真机拨弦验证。

2026-10-05 使用固定 Rust 1.95.0 MSVC 工具链执行本机正式模式测试，57 个测试全部通过，包括新增的 13 个选弦规格。cargo fmt --check 通过。选弦测试检查状态、权限与录音意图，避免依赖按钮和提示的具体文案。

独立变异验证：只修改按钮与状态文案时仍 57 个测试通过；删除正在录音判断后 55 通过、2 失败，删除权限等待判断后 56 通过、1 失败，删除前台判断后 56 通过、1 失败。测试能够拦住权限处理中重复启动、后台启动及录音中换弦触发开始/停止切换的问题。变异仅在临时副本中执行，副本已清理。

GitHub Actions 执行主机测试和三个 Android 目标的 cargo check；结果需以对应提交的实际任务结果为准。cargo check 不链接或生成 Android 本机库，不代替正式 APK 打包与 JNI 运行验证。

## 正式 APK 验证

本机执行 :app:lintRelease :app:assembleRelease 成功，arm64-v8a、armeabi-v7a、x86_64 三个架构均完成正式编译和链接。Android Lint 报告为 No issues found。

aapt 从真实 APK 清单读取到 com.ofairyo.xianzhun、0.4.10-locked-string、版本代码 14、最低 API 26、目标与编译 API 36。APK 包含三个架构的 Rust 本机库，均导出 ANativeActivity_onCreate，没有 DEX。包内三个库的 SHA-256 与本次编译输出一致，并包含选弦诊断、0.4.10 版本和既有录音恢复、窗口绘制修复标记。

正式签名验证通过，证书 SHA-256 为 f89452442d3a823088359f579961c5cfc84508cbcc8e4681bfa0045dbfc7e2ad，与旧版一致。zipalign -c -P 16 4 通过，64 位 ELF LOAD 段采用 0x4000 对齐。

交付文件为 xianzhun-release-v0.4.10-locked-string.apk，1356298 字节，SHA-256 为 57FBF267D101BB11CE690BB09A69D9F91AB403395B5E9F9134C66203531748F1。根目录与构建输出副本哈希一致，旧 0.4.9 APK 归档到 old_apks。

## 本地复核命令

先运行 Rust 领域测试和格式检查，再用既有签名配置执行正式 Android 构建，不绕过证书校验，不提交签名材料。

```powershell
cargo +stable-x86_64-pc-windows-msvc test --locked --manifest-path rust/Cargo.toml --release
cargo +stable-x86_64-pc-windows-msvc fmt --manifest-path rust/Cargo.toml --check
.\gradlew.bat :app:lintRelease :app:assembleRelease --offline --no-daemon --console=plain
```

## 真机验收（尚未执行）

1. 首次打开不点开始，直接锁定任意弦。未授权时正常弹出授权；允许后保留该弦并进入录音。已经授权时直接开始。分别拨六根对应空弦，观察频率、指针与音分。
2. 录音中连续换弦，确认停止按钮保持“停止调音”，没有停止收音、重复占用或沿用上一根弦的已调准状态。点同一弦取消锁定后，应继续自动识别。
3. 主动停止后取消锁定不得重新录音；重新选择另一根弦可开始。权限拒绝/永久拒绝、麦克风不可用、切后台和返回时，提示与实际收音状态一致。
4. 对比低/高于目标的音分方向；拨错弦或疑似低弦泛音不得被直接当成当前锁定弦已调准。本次不放宽识别阈值。
5. 若按钮已显示“停止调音”却仍无读数，记录目标弦、手机型号、系统版本和以下日志，继续排查真实音频输入/识别路径，不把本次启动意图修复误当成所有无读数问题都已解决。

```powershell
adb logcat -s XianZhun AudioRecord System.err
```
