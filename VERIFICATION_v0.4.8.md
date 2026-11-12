0.4.8-startup-fix - 启动黑屏修复验证记录

验证日期：2026-10-05。应用包名：com.ofairyo.xianzhun。版本名称：0.4.8-startup-fix。版本代码：12。

## 黑屏原因与修复

0.4.7 使用系统 NativeActivity。其 onCreate 在加载 Rust 本机入口之前调用 Window.takeSurface(this) 和 takeInputQueue(this)。系统因此把绘制和输入交给原生应用；ViewRoot 不会绘制被接管 Surface 上的 View 层级。应用却只创建 Android View 控件，没有向 ANativeWindow 提交画面，也未解除接管，存在直接导致黑屏的启动路径。该行为在当前 AOSP 和 Android 8.0 中均存在。

官方依据：[Window.takeSurface 文档](https://developer.android.com/reference/android/view/Window#takeSurface(android.view.SurfaceHolder.Callback2))、[NativeActivity 源码](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/app/NativeActivity.java)、[ViewRootImpl 源码](https://raw.githubusercontent.com/aosp-mirror/platform_frameworks_base/master/core/java/android/view/ViewRootImpl.java)。

本版在 ANativeActivity_onCreate 中、create_ui 和首次 ViewRoot 挂载前调用 takeSurface(null) 与 takeInputQueue(null)，交回框架的绘制与触控分发。首次帧回调重复 setContentView 的逻辑已移除。独立只读审查确认调用顺序、JNI 签名和局部引用生命周期正确。

启动窗口设置、活动引用创建或控件创建失败时，会记录错误阶段及 Java 异常字符串，日志标签为 XianZhun。成功完成界面创建后记录 startup ui-ready v0.4.8。控件创建失败时尝试显示错误文字；窗口接口自身失败时不能保证提示可见。成功日志代表控件初始化完成，不是已绘制到手机屏幕的证明。

## 已执行验证

Rust 1.95.0 MSVC 正式工具链的 34 个现有领域与状态测试通过，cargo fmt --check 通过。这些测试检查音高和业务状态，不检查 Android 窗口绘制；本次未增加复制实现的窗口单元测试。0.4.7 的领域规格与变异证据见 VERIFICATION_v0.4.7.md。

Gradle 执行 :app:lintRelease :app:assembleRelease --offline --no-daemon --console=plain 成功。arm64-v8a、armeabi-v7a、x86_64 三个 Rust 本机库均编译成功。Android Lint 报告为 No issues found。

aapt 从打包后的 AndroidManifest.xml 读取到 com.ofairyo.xianzhun、版本代码 12、版本名 0.4.8-startup-fix、最低 API 26、目标与编译 API 36。包内包含三个架构的 libxianzhun_tuner.so，各架构均导出 ANativeActivity_onCreate，没有 DEX。应用实现保持 Rust。

直接读取交付 APK 内三个本机库，均包含 takeSurface、takeInputQueue 和 startup ui-ready v0.4.8 标记，且包内本机库 SHA-256 与本次编译输出一致，确认正式包包含此次修复。

apksigner 校验成功，证书 SHA-256 为 f89452442d3a823088359f579961c5cfc84508cbcc8e4681bfa0045dbfc7e2ad，与原正式版本一致。zipalign -c -P 16 4 检查成功，64 位 ELF LOAD 段采用 0x4000 对齐。

交付文件：xianzhun-release-v0.4.8-startup-fix.apk。大小：1352614 字节。文件 SHA-256：BC9E78699D6F2399B2ADAE12BD662F183A98633205CAE4184E771F58552BB420。根目录交付副本与构建输出的哈希一致；旧正式版本归档到 old_apks。

## 验证范围

已修复源码中确定的窗口接管错误，完成静态核查、领域测试与正式打包。尚未在用户手机上确认启动画面，也未采集该手机的运行日志，不能排除额外的厂商或设备问题。未创建、启动或调用安卓模拟器，未操作未经授权的真机，未更改 VPN。

仅交付带版本号的签名正式 APK；未生成 debug APK、xiaomi debug APK、app-release APK 或 AAB。
