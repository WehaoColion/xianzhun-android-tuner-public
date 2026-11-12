0.4.7-performance-ui - 性能与稳定性修复验证记录

验证日期：2026-10-05。应用包名：com.ofairyo.xianzhun。版本名称：0.4.7-performance-ui。版本代码：11。

## 功能与领域验证

使用正式构建所固定的 Rust 1.95.0 MSVC 工具链执行 `cargo +stable-x86_64-pc-windows-msvc test --manifest-path rust/Cargo.toml --offline --release`，34 个测试全部通过，其中音高和选弦领域测试 27 个、界面状态测试 7 个。`cargo fmt --check` 通过。

测试覆盖六根空弦、弱基频和强谐波、噪声及直流偏置、越界和无效输入、自动选弦边界、锁定弦的泛音冲突、连续三帧调准确认、低置信度中断、读数过期、目标切换、暂停和权限等待状态。新增低频及高采样率短帧规格均可复现旧版错误并验证修复。

文案变异后，27 个音高领域测试和 7 个界面状态测试保持通过。删除分类入口有效性判断时，低置信度和零音量测试失败；删除未闭合周期判断时，高采样率短帧测试失败；删除连续三帧确认判断时，三个状态测试失败。临时变异源码与可执行文件已清理。

## 算法性能与数值复核

同一 GNU Rust 1.95.0 编译器、同一机器、相同优化选项的交错对比中，2400 个合成六弦 PCM 帧从 2578.530 ms 降至 434.951 ms，分析耗时减少约 83.1%，吞吐约为原来的 5.93 倍。固定帧预热后堆分配计数为 0。该数据是主机算法工作量对比，不是 Android 整机运行或耗电测试。完整工具链、复现命令和其他测量记录见 rust/tools/PITCH_BENCHMARK.md。

1008 帧旧版与新版对照检出情况一致，最大旧新音准差为 0.000413 音分，在对应合成条件下新版相对生成音高的最大误差为 0.869670 音分。独立复核另通过 78 个边界案例、360 个生产采样率合成案例、336 个缓存复用案例和 128 个频谱功率数值案例。实际采集仍使用 44.1/48 kHz；高采样率案例验证公开算法接口的边界拒绝。

## Android 构建与包验证

Gradle 8.11.1、Android Gradle Plugin 8.9.1、cargo-ndk 4.1.2、Android NDK 27.1.12297006。`:app:lintRelease :app:assembleRelease` 成功，Android Lint 报告为 `No issues found`。

APK 包内版本通过 aapt 从真实 AndroidManifest.xml 读取。最低 Android API 为 26，目标 API 为 36。包内包含 arm64-v8a、armeabi-v7a、x86_64 三个架构的本机库，均导出 ANativeActivity_onCreate；没有 DEX。新增 JNI 控件调用签名已与本机 SDK 的 android.jar 核对。

apksigner 校验成功，沿用原正式签名，证书 SHA-256 为 f89452442d3a823088359f579961c5cfc84508cbcc8e4681bfa0045dbfc7e2ad。zipalign 的 `-c -P 16 4` 校验成功；arm64-v8a 和 x86_64 的 ELF LOAD 段采用 0x4000 对齐。

未改变源码时再次执行正式构建成功，buildRustRelease 显示 UP-TO-DATE，正式签名核验仍执行。构建已添加 Rust 源码、依赖锁文件、固定工具版本和 NDK 信息的输入追踪。

交付文件：xianzhun-release-v0.4.7-performance-ui.apk。大小：1325194 字节。文件 SHA-256：8BA950E62A181AE33269D78565CA933779801976D94F7E5DA90CEC319E7F0743。

## 验证范围

本次完成静态检查、主机领域测试、合成音频数值对照及正式打包。未启动安卓模拟器，未连接或操作未经授权的真机。实际麦克风资源释放、厂商音频调用、触控、系统栏、小屏及大字体显示效果仍需真机验证。厂商音频调用可能延迟返回，本次修复使主线程不再等待该调用，并保留旧录音任务直到释放资源。

本次应用实现使用 Rust。仅保留带版本号的签名正式 APK；旧正式版本归档到 old_apks。
