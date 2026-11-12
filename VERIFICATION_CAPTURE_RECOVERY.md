0.4.9-capture-recovery - 录音恢复修复与正式 APK 验证记录。

# 录音恢复验证

基线：`8d207af44639a5f19c809c6514220cdc7bdc4dfa`，`0.4.8-startup-fix`。

## 已定位的代码故障路径

原实现仅在 `AudioRecord.getState() == STATE_INITIALIZED` 后选择配置，随后在独立步骤调用 `startRecording()`。因此，首选音源能构造但不能开始录音时，既有音源和采样率回退不会执行。`read()` 返回负值时同样直接结束会话。

非阻塞 `read()` 持续返回 0 时，原实现只有休眠和重试，没有完整 PCM 帧的超时判断。少量样本持续返回但长期无法凑满一帧时，也可能一直没有读数。`getMinBufferSize()` 的 JNI 异常还会提前退出所有配置尝试。

这些是源码中确认的故障路径，不代表已经通过用户手机日志确认了此次故障的唯一原因。

## 本次修改

- 将初始化、实际启动、读取和超时统一纳入同一回退过程，沿用 48 kHz / 44.1 kHz 与 UNPROCESSED / VOICE_RECOGNITION / MIC 的原尝试顺序。每个配置每次会话至多尝试一次。
- 只有初始化和实际录音状态均有效，才进入采样；用 `getSampleRate()` 的实际采样率计算音高。
- 连续 2 秒没有完整 PCM 帧则释放当前录音器并尝试下一个配置。有效静音帧仍算采样进展，不能因为未拨弦或未识别出音高就切换音源。
- 失败配置先停止并释放，再创建下一个录音器；停止、暂停和退出仍遵守原有会话标志与单录音线程约束。全部配置失败后显示现有“重试录音”状态，不无限重启。
- 记录音源、请求/实际采样率、启动失败、读取错误和超时信息，不记录 PCM 音频。

正式打包版本为 0.4.9-capture-recovery，版本代码 13，沿用原应用身份和正式签名。界面布局、音高阈值和调准判据不变。APK 作为 GitHub Release 附件分发；历史 APK 保留在本地 old_apks。

## 自动化测试状态

新增 `rust/src/capture.rs` 中 10 个主机端测试，覆盖启动/读取失败回退、采样率回退、配置耗尽、取消时序、空读及不完整帧超时、静音完整帧持续采集。

2026-10-05 使用固定 Rust 1.95.0 MSVC 工具链执行正式模式主机测试，44 个测试全部通过，包含上述 10 个录音恢复测试。cargo fmt --check 通过。测试中的录音尝试使用可控回调；它们验证恢复策略，不等同于 Android JNI 集成测试。

独立变异验证：原始录音策略测试 10 通过；只改模块注释后仍 10 通过。删除尝试前 active() 判断后 8 通过、2 失败，停止前与失败后取消两条规格拦住错误。将超时判断改为恒 false 后 7 通过、3 失败，空读、零碎帧和完整帧之后的超时规格拦住错误。临时变异副本已清理，正式源码未保留变异。

只读审查确认，录音对象在下一次配置尝试前由原工作线程停止并释放，JNI 局部引用在每次尝试后回收，音高分析使用实际采样率。AudioRecord 新增调用签名已与本机 API 36 的 android.jar 核对。

## 正式 APK 验证

2026-10-05 正式执行 :app:lintRelease :app:assembleRelease 成功，arm64-v8a、armeabi-v7a、x86_64 三个架构均编译通过。Android Lint 报告为 No issues found。

aapt 从真实 APK 清单读取到包名 com.ofairyo.xianzhun、版本名 0.4.9-capture-recovery、版本代码 13、最低 API 26、目标与编译 API 36。APK 内有三个架构的 Rust 本机库，均导出 ANativeActivity_onCreate，没有 DEX。

apksigner 校验通过，证书 SHA-256 为 f89452442d3a823088359f579961c5cfc84508cbcc8e4681bfa0045dbfc7e2ad，与原正式版一致。zipalign -c -P 16 4 通过；64 位 ELF LOAD 段采用 0x4000 对齐。

包内三个本机库与本次编译输出 SHA-256 一致，均包含 0.4.9 启动标记、实际采样率和完整帧超时逻辑标记，确认正式安装包包含当前修复。

交付文件为 xianzhun-release-v0.4.9-capture-recovery.apk，1354318 字节，SHA-256 为 41DF53F5948CDB5CF626AA93BADC93E496B9259B62A4666BAC3BCE79C40C4173。项目根目录与构建输出副本哈希一致，0.4.8 正式版归档到 old_apks。

## 本地复核命令

先确认工具链与 README 的固定构建版本一致。Android JNI 集成由正式构建同时编译三个架构检查。

```powershell
cargo +stable-x86_64-pc-windows-msvc test --locked --manifest-path rust/Cargo.toml --release
cargo +stable-x86_64-pc-windows-msvc fmt --manifest-path rust/Cargo.toml --check
.\gradlew.bat :app:lintRelease :app:assembleRelease --offline --no-daemon --console=plain
```

普通主机测试不会编译 native_app.rs；必须执行 Android 正式构建。按 README 准备正式签名，不绕过证书校验，不提交签名材料。

## 真机验收（尚未执行）

1. 首次安装授权麦克风后开始调音；分别在自动及手动锁定模式拨六根空弦，确认读数、音分方向、停止按钮有效。
2. 静置至少 15 秒再拨弦，确认正常静音不会造成音源反复切换。
3. 在可控测试版本或设备故障场景中，验证首选配置初始化成功但启动失败、负值读取、持续空读及不完整帧超时后能够尝试后续配置；全部失败后可以手动重试。不要将故障注入保留在正式代码中。
4. 采样及回退期间反复停止/开始、切后台、返回、退出，确认没有重复麦克风占用、后台继续采音、旧线程恢复已清除读数的问题。
5. 分别验证系统麦克风隐私开关、权限拒绝/撤回、其他应用占用。系统返回完整的全零帧并不等于传输超时，本补丁不会绕过系统隐私控制。
6. 复现仍无法调音时保存设备型号、Android 版本、界面状态及日志：

```powershell
adb logcat -s XianZhun AudioRecord System.err
```

2 秒看门狗约束完整音频帧的停滞，不保证厂商的初始化、启动或释放 JNI 调用自身绝不阻塞。实际手机启动、麦克风采集和故障恢复仍未真机验证；未创建、启动或调用安卓模拟器。
