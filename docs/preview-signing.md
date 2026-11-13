# 预览版 APK 的长期签名与覆盖安装

## 原因

早期预览 APK 使用 GitHub Actions 中自动生成的 Android Debug 证书。每次 Windows Runner 的临时密钥不同，导致相同包名 `com.ofairyo.xianzhun.preview` 的 APK 无法覆盖更新。0.4.12 与 0.4.13 的预览证书不同，历史包不具备长期连续更新保证。

Android 更新安装要求新旧 APK 的包名一致，且签名证书相同（或满足系统支持的签名轮换规则）。已经丢失的旧私钥不能从签名证书中恢复，因此切换到固定预览证书时，需要**仅卸载一次旧预览版**。正式版 `com.ofairyo.xianzhun` 不受影响。

## 一次性安全设置

项目维护人需要将独立生成并离线备份的预览版私钥导入 GitHub Actions Secrets。**严禁把 keystore、Base64 私钥、口令或完整 Secret 内容提交到公开仓库、PR、Issue、日志或 Wiki。**

1. 在离线保存的 `弦准-长期签名密钥-仅本人保存.zip` 中找到 `github-secret-value.txt`。它是一行 JSON，含 `keystoreBase64`、`storePassword`、`keyAlias` 和 `keyPassword` 四个字段。
2. 打开 [公开仓库的 Actions Secrets 设置](https://github.com/WehaoColion/xianzhun-android-tuner-public/settings/secrets/actions)，选择 **New repository secret**。
3. 名称填写 `XIANZHUN_PREVIEW_SIGNING_JSON`，值粘贴上述文本文件的完整内容。不要粘贴到聊天或公开文件。
4. 保存后，重新运行 PR 的 `Android preview APK` 工作流。密钥仅存在于 Runner 的临时文件中，构建结束自动清理。

## 固定身份

| 属性 | 值 |
| --- | --- |
| 预览版包名 | `com.ofairyo.xianzhun.preview` |
| 签名证书 SHA-256 | `24523de06f8f020aa37f4c91721f5e30ebd9dd84f14ac9ad0692401c5cb34e76` |
| 密钥别名 | `xianzhun-preview` |
| 正式版包名 | `com.ofairyo.xianzhun`，沿用原正式签名 |
| APK 类型 | 可安装测试预览包，不是正式 Release |

Gradle 构建图发现预览 APK 打包任务时会强制要求长期签名；若密钥缺失、证书不匹配或校验任务被明确排除，立即构建失败，**禁止回退为临时 Debug 签名**。APK 生成后，还会使用 Android Build Tools 的 `apksigner` 校验最终 APK 中的真实证书指纹，验证包名与三个 ABI，成功后才上传可下载安装包。

## 更新步骤与注意事项

首次切换固定签名：卸载旧的「弦准预览版」，再安装第一次使用固定证书签署的预览 APK。此后继续使用同一应用 ID 和这把固定密钥，并确保下一 APK 的 versionCode 不小于原安装版本，即可直接覆盖更新，无需卸载。

新的 GitHub Runner 不会生成新密钥。若证书丢失、Secret 被修改、应用 ID 更改或 versionCode 倒退，将再次破坏正常更新路径，因此本仓库的证书 SHA-256 进行双重固定校验。

**不要把正式版私钥用于公开 PR 的 CI。** 对来自 Fork 的不受信任 PR，预览 APK 签名任务被跳过，避免暴露密钥。备份 ZIP 含有真实私钥，请本人离线加密保存，丢失无法恢复；若泄露应按安全事件处理。
