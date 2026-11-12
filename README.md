0.4.12-instrument-ui - 重构调音仪表、显式自动识别和固定底部操作区。

# 弦准

Rust 编写的古典吉他安卓调音器，支持标准六弦 E2、A2、D3、G3、B3、E4，显示音高、音分偏差和调准状态，可自动选弦或手动锁定目标弦。

支持 Android 8.0（API 26）及以上版本，目标 API 36。应用仅申请麦克风权限，音频在设备内处理。

## 安装

从 [Releases](https://github.com/WehaoColion/xianzhun-android-tuner-public/releases) 下载带版本号的正式 APK。目前已发布的正式 APK 为 `0.4.11-microphone-input`。源码版本为 `0.4.12-instrument-ui`，尚未发布对应签名 APK，不能将开发分支当作已发布版本。

0.4.12 使用固定底部操作按钮、非交互式音准刻度和动态指针，新增显式自动识别入口，并把输入波形状态与音高结果分开显示；无有效读数时隐藏指针，避免误示已调准。尚未进行本版真机验收。

0.4.11 优先使用普通麦克风输入，低增益下的微弱空弦仍可识别。输入电平独立于音高结果，区分启动、真实波形、数字静音和系统静音；有声音但没有有效琴弦音高时也能看到电平。持续常量 PCM 会尝试其他配置，保持可用录音器等待拨弦；系统静音时显示明确提示并等待恢复。此前锁定弦收音、停止取消和启动绘制修复保留。真机收音结果尚未验证，记录见 [VERIFICATION_MICROPHONE_INPUT.md](VERIFICATION_MICROPHONE_INPUT.md)。

## 项目结构

| 路径 | 内容 |
| --- | --- |
| `rust/src/pitch.rs` | 音高分析与信号有效性判断 |
| `rust/src/capture.rs` | 录音配置回退与完整帧超时策略 |
| `rust/src/input.rs` | 去直流的原始输入电平和波形状态 |
| `rust/src/tuning.rs` | 标准调弦、音分和选弦逻辑 |
| `rust/src/ui_state.rs` | 权限、读数、目标弦与调准状态 |
| `rust/src/selection.rs` | 选弦事务与录音启动意图 |
| `rust/src/native_app.rs` | Android 生命周期、JNI 控件和音频采集 |
| `app/src/main/` | 清单、主题和图标等 Android 资源 |
| `app/build.gradle.kts` | 原生编译、正式签名与 APK 校验 |
| `rust/tools/` | 音高性能基准与验证记录 |

应用逻辑使用 Rust，通过 JNI 调用系统控件与音频接口；没有 Java 或 Kotlin 应用源码。Gradle Kotlin DSL 用于构建配置。

## 构建环境

当前正式构建固定使用以下版本，工具校验不匹配时会停止构建。

| 工具 | 版本 |
| --- | --- |
| 主机 | Windows x64 |
| JDK | 17 |
| Gradle | 8.11.1，通过仓库 Wrapper 调用 |
| Android Gradle Plugin | 8.9.1 |
| Rust | 1.95.0，`stable-x86_64-pc-windows-msvc` |
| cargo-ndk | 4.1.2 |
| Android SDK / Build Tools | 36 / 36.0.0 |
| Android NDK | 27.1.12297006 |

本机 Rust 稳定工具链必须与脚本固定的 1.95.0 版本一致，直接升级稳定工具链可能触发版本校验失败。Rust 目标需安装 `aarch64-linux-android`、`armv7-linux-androideabi`、`x86_64-linux-android`；Windows 主机还需可用的 MSVC 链接器。

配置 `JAVA_HOME`、`ANDROID_SDK_ROOT`，并将 Cargo / rustup 放入 PATH。也可在本地 `local.properties` 配置 `sdk.dir`、`ndk.dir`、`cargo.path`、`rustup.path`、`cargo.ndk.path`；这些本机配置不纳入仓库。

首次构建先下载依赖，原生打包阶段使用 Cargo 离线模式：

```powershell
cargo +stable-x86_64-pc-windows-msvc fetch --locked --manifest-path rust/Cargo.toml
cargo +stable-x86_64-pc-windows-msvc test --manifest-path rust/Cargo.toml --release
cargo +stable-x86_64-pc-windows-msvc fmt --manifest-path rust/Cargo.toml --check
```

正式签名通过本机环境变量提供：`XIANZHUN_RELEASE_STORE_FILE`、`XIANZHUN_RELEASE_STORE_PASSWORD`、`XIANZHUN_RELEASE_KEY_ALIAS` 和 `XIANZHUN_RELEASE_KEY_PASSWORD`。仓库不包含密钥或密码。构建会核对既有正式证书指纹，缺少签名材料或证书不匹配时终止；使用不同应用身份或证书需要自行调整构建配置。

```powershell
.\gradlew.bat :app:lintRelease :app:assembleRelease
```

正式产物位于 `app/build/outputs/apk/release/xianzhun-release-v0.4.12-instrument-ui.apk`，包含 arm64-v8a、armeabi-v7a 和 x86_64 三个架构。构建限制仅允许带版本号的签名正式 APK，不生成 debug APK 或 AAB。

## 版本与验证

[RELEASE_NOTES.md](RELEASE_NOTES.md) 记录版本变更。当前已有 57 个选弦、录音恢复、音高和业务状态规格测试；窗口绘制、麦克风和触控的实际表现需在授权的真机上验证。项目不使用安卓模拟器。

APK 通过 Releases 单独分发。历史 APK 可从历史版本 Release 下载；本机日志、签名材料、原生二进制中间产物和构建缓存不纳入源码仓库。
