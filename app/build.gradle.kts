import com.android.build.api.dsl.ApplicationExtension
import com.android.build.gradle.AppExtension
import com.android.build.gradle.internal.api.BaseVariantOutputImpl
import java.io.File
import java.security.KeyStore
import java.security.MessageDigest
import java.util.Properties
import org.gradle.api.GradleException

apply(plugin = "com.android.application")

val appVersionName = "0.4.13-warm-brass"
val requiredCargoNdkVersion = "cargo-ndk 4.1.2"
val rustToolchain = System.getenv("XIANZHUN_RUST_TOOLCHAIN")
    ?.takeIf { it.isNotBlank() }
    ?: "stable-x86_64-pc-windows-msvc"
val requiredRustcVersion = "rustc 1.95.0 (59807616e 2026-04-14)"
val androidBuildToolsVersion = "36.0.0"
val expectedReleaseSignerSha256 =
    "f89452442d3a823088359f579961c5cfc84508cbcc8e4681bfa0045dbfc7e2ad"
val localProperties = Properties().apply {
    val localFile = rootProject.file("local.properties")
    if (localFile.isFile) {
        localFile.inputStream().use(::load)
    }
}

fun configuredValue(propertyName: String, environmentName: String): String? {
    return localProperties.getProperty(propertyName)?.takeIf { it.isNotBlank() }
        ?: System.getenv(environmentName)?.takeIf { it.isNotBlank() }
}

fun configuredFile(path: String): File {
    val candidate = File(path)
    return if (candidate.isAbsolute) candidate else rootProject.file(path)
}

val isWindowsHost = System.getProperty("os.name").startsWith("Windows", ignoreCase = true)
val cargoHome = configuredValue("cargo.home", "CARGO_HOME")?.let(::configuredFile)
val rustupHome = configuredValue("rustup.home", "RUSTUP_HOME")?.let(::configuredFile)
val explicitCargoPath = configuredValue("cargo.path", "CARGO")?.let(::configuredFile)
val explicitRustupPath = configuredValue("rustup.path", "RUSTUP")?.let(::configuredFile)
val explicitCargoNdkPath = configuredValue("cargo.ndk.path", "CARGO_NDK")?.let(::configuredFile)
val sdkRoot = configuredValue("sdk.dir", "ANDROID_SDK_ROOT")
    ?.let(::configuredFile)
    ?: System.getenv("ANDROID_HOME")?.takeIf { it.isNotBlank() }?.let(::configuredFile)
val ndkRoot = localProperties.getProperty("ndk.dir")?.takeIf { it.isNotBlank() }
    ?.let(::configuredFile)
    ?: System.getenv("ANDROID_NDK_HOME")?.takeIf { it.isNotBlank() }?.let(::configuredFile)
    ?: System.getenv("ANDROID_NDK_ROOT")?.takeIf { it.isNotBlank() }?.let(::configuredFile)
    ?: sdkRoot?.resolve("ndk/27.1.12297006")
val rustProjectDir = rootProject.file("rust")
val debugJniOutDir = project.file("src/debug/jniLibs")
val releaseJniOutDir = project.file("src/release/jniLibs")
val previewJniOutDir = project.file("src/preview/jniLibs")

fun executableNames(tool: String): List<String> {
    return if (isWindowsHost) {
        listOf("$tool.exe", "$tool.cmd", "$tool.bat", tool)
    } else {
        listOf(tool)
    }
}

fun executableFromDirectory(directory: File?, tool: String): File? {
    return directory?.takeIf(File::isDirectory)?.let { existingDirectory ->
        executableNames(tool)
            .asSequence()
            .map(existingDirectory::resolve)
            .firstOrNull { it.isFile && (isWindowsHost || it.canExecute()) }
    }
}

fun executableOnPath(tool: String): File? {
    return System.getenv("PATH")
        ?.split(File.pathSeparatorChar)
        ?.asSequence()
        ?.map(String::trim)
        ?.map { it.trim('"') }
        ?.filter(String::isNotEmpty)
        ?.map(::File)
        ?.mapNotNull { executableFromDirectory(it, tool) }
        ?.firstOrNull()
}

fun requireConfiguredExecutable(explicitPath: File?, label: String): File? {
    if (explicitPath == null) {
        return null
    }
    if (!explicitPath.isFile || (!isWindowsHost && !explicitPath.canExecute())) {
        throw GradleException(
            "$label executable does not exist or is not executable: ${explicitPath.absolutePath}",
        )
    }
    return explicitPath
}

fun resolveCargoExecutable(): File {
    requireConfiguredExecutable(explicitCargoPath, "Cargo")?.let { return it }
    executableFromDirectory(cargoHome?.resolve("bin"), "cargo")?.let { return it }
    executableOnPath("cargo")?.let { return it }
    throw GradleException(
        "Cargo was not found. Set cargo.path in local.properties, set the CARGO environment " +
            "variable to the cargo executable, configure cargo.home/CARGO_HOME, or add cargo to PATH.",
    )
}

fun resolveRustupExecutable(): File? {
    requireConfiguredExecutable(explicitRustupPath, "Rustup")?.let { return it }
    executableFromDirectory(cargoHome?.resolve("bin"), "rustup")?.let { return it }
    return executableOnPath("rustup")
}

fun resolveCargoNdkExecutable(cargoExecutable: File): File {
    requireConfiguredExecutable(explicitCargoNdkPath, "cargo-ndk")?.let { return it }
    executableFromDirectory(cargoHome?.resolve("bin"), "cargo-ndk")?.let { return it }
    executableFromDirectory(cargoExecutable.parentFile, "cargo-ndk")?.let { return it }
    executableOnPath("cargo-ndk")?.let { return it }
    throw GradleException(
        "cargo-ndk $requiredCargoNdkVersion was not found. Set cargo.ndk.path/CARGO_NDK, " +
            "configure cargo.home/CARGO_HOME, or add its bin directory to PATH.",
    )
}

fun requireSdkRoot(): File {
    return sdkRoot?.takeIf(File::isDirectory) ?: throw GradleException(
        "Android SDK was not found. Set sdk.dir in local.properties, ANDROID_SDK_ROOT, or ANDROID_HOME.",
    )
}

fun requireNdkRoot(): File {
    return ndkRoot?.takeIf(File::isDirectory) ?: throw GradleException(
        "Android NDK 27.1.12297006 was not found. Set ndk.dir in local.properties, " +
            "ANDROID_NDK_HOME, or ANDROID_NDK_ROOT, or install it under the configured SDK.",
    )
}

fun toolPath(
    cargoExecutable: File,
    rustupExecutable: File?,
    cargoNdkExecutable: File,
): String {
    return buildList {
        add(cargoExecutable.parentFile.absolutePath)
        add(cargoNdkExecutable.parentFile.absolutePath)
        rustupExecutable?.parentFile?.absolutePath?.let(::add)
        System.getenv("PATH")?.takeIf { it.isNotBlank() }?.let(::add)
    }.distinct().joinToString(File.pathSeparator)
}

data class CommandResult(val exitCode: Int, val output: String)

fun runCommand(command: List<String>, environment: Map<String, String> = emptyMap()): CommandResult {
    val processBuilder = ProcessBuilder(command)
        .directory(rootProject.projectDir)
        .redirectErrorStream(true)
    processBuilder.environment().putAll(environment)
    val process = processBuilder.start()
    val output = process.inputStream.bufferedReader().use { it.readText() }
    return CommandResult(process.waitFor(), output.trim())
}

fun cargoToolEnvironment(
    cargoExecutable: File,
    rustupExecutable: File?,
    cargoNdkExecutable: File,
): Map<String, String> = buildMap {
    put("PATH", toolPath(cargoExecutable, rustupExecutable, cargoNdkExecutable))
    cargoHome?.let { put("CARGO_HOME", it.absolutePath) }
    rustupHome?.let { put("RUSTUP_HOME", it.absolutePath) }
}

fun verifyPinnedNativeTools() {
    val cargoExecutable = resolveCargoExecutable()
    val rustupExecutable = resolveRustupExecutable()
        ?: throw GradleException("rustup is required to verify the pinned Rust toolchain.")
    val cargoNdkExecutable = resolveCargoNdkExecutable(cargoExecutable)
    val environment = cargoToolEnvironment(
        cargoExecutable,
        rustupExecutable,
        cargoNdkExecutable,
    )
    val cargoNdkResult = runCommand(
        listOf(cargoExecutable.absolutePath, "+$rustToolchain", "ndk", "--version"),
        environment,
    )
    if (
        cargoNdkResult.exitCode != 0 ||
        cargoNdkResult.output.lineSequence().lastOrNull() != requiredCargoNdkVersion
    ) {
        throw GradleException(
            "Expected $requiredCargoNdkVersion, but cargo ndk reported: ${cargoNdkResult.output}",
        )
    }
    val rustcResult = runCommand(
        listOf(
            rustupExecutable.absolutePath,
            "run",
            rustToolchain,
            "rustc",
            "--version",
        ),
        environment,
    )
    if (rustcResult.exitCode != 0 || rustcResult.output != requiredRustcVersion) {
        throw GradleException(
            "Expected $requiredRustcVersion, but the pinned toolchain reported: " +
                rustcResult.output,
        )
    }
}

fun signingValue(name: String): String? {
    return System.getenv(name)?.takeIf { it.isNotBlank() }
}

val releaseKeyStore = signingValue("XIANZHUN_RELEASE_STORE_FILE")
    ?.takeIf { it.isNotBlank() }
    ?.let(::file)
    ?: File(File(System.getProperty("user.home"), ".android"), "xianzhun-release.jks")
val releaseKeyPassword = signingValue("XIANZHUN_RELEASE_KEY_PASSWORD")
val releaseStorePassword = signingValue("XIANZHUN_RELEASE_STORE_PASSWORD")
val releaseKeyAlias = signingValue("XIANZHUN_RELEASE_KEY_ALIAS")
    ?.takeIf { it.isNotBlank() }
    ?: "xianzhun"
val hasReleaseSigning = releaseKeyStore.isFile &&
    releaseKeyAlias.isNotBlank() &&
    !releaseKeyPassword.isNullOrBlank() &&
    !releaseStorePassword.isNullOrBlank()

fun configuredReleaseCertificateSha256(): String {
    val password = releaseStorePassword
        ?: throw GradleException("The release keystore password is unavailable.")
    var lastFailure: Exception? = null
    for (storeType in listOf("PKCS12", "JKS")) {
        try {
            val keyStore = KeyStore.getInstance(storeType)
            releaseKeyStore.inputStream().use { input ->
                keyStore.load(input, password.toCharArray())
            }
            val certificate = keyStore.getCertificate(releaseKeyAlias)
                ?: throw GradleException(
                    "Release alias '$releaseKeyAlias' is missing from ${releaseKeyStore.absolutePath}.",
                )
            return MessageDigest.getInstance("SHA-256")
                .digest(certificate.encoded)
                .joinToString("") { byte -> "%02x".format(byte.toInt() and 0xff) }
        } catch (failure: Exception) {
            lastFailure = failure
        }
    }
    throw GradleException(
        "Unable to read the release certificate from ${releaseKeyStore.absolutePath}.",
        lastFailure,
    )
}

fun verifyGeneratedReleaseApkSigner() {
    val apk = layout.buildDirectory
        .file("outputs/apk/release/xianzhun-release-v$appVersionName.apk")
        .get()
        .asFile
    if (!apk.isFile) {
        throw GradleException("Expected release APK is missing: ${apk.absolutePath}")
    }
    val apkSignerJar = requireSdkRoot()
        .resolve("build-tools/$androidBuildToolsVersion/lib/apksigner.jar")
    val javaExecutable = File(
        System.getProperty("java.home"),
        if (isWindowsHost) "bin/java.exe" else "bin/java",
    )
    if (!apkSignerJar.isFile || !javaExecutable.isFile) {
        throw GradleException("Pinned apksigner or Java runtime is unavailable.")
    }
    val result = runCommand(
        listOf(
            javaExecutable.absolutePath,
            "-jar",
            apkSignerJar.absolutePath,
            "verify",
            "--verbose",
            "--print-certs",
            apk.absolutePath,
        ),
    )
    if (result.exitCode != 0) {
        throw GradleException("Release APK signature verification failed: ${result.output}")
    }
    val signer = Regex("Signer #1 certificate SHA-256 digest: ([0-9a-fA-F]{64})")
        .find(result.output)
        ?.groupValues
        ?.get(1)
        ?: throw GradleException("Release APK signer digest was not reported by apksigner.")
    if (!signer.equals(expectedReleaseSignerSha256, ignoreCase = true)) {
        throw GradleException(
            "Generated APK signer mismatch. Expected $expectedReleaseSignerSha256 but found " +
                signer.lowercase() + ".",
        )
    }
}

configure<ApplicationExtension> {
    namespace = "com.ofairyo.xianzhun"
    compileSdk = 36
    buildToolsVersion = androidBuildToolsVersion

    defaultConfig {
        applicationId = "com.ofairyo.xianzhun"
        minSdk = 26
        targetSdk = 36
        versionCode = 17
        versionName = appVersionName

        vectorDrawables {
            useSupportLibrary = true
        }
        ndk {
            abiFilters.addAll(listOf("arm64-v8a", "armeabi-v7a", "x86_64"))
        }
    }

    signingConfigs {
        if (hasReleaseSigning) {
            create("release") {
                storeFile = releaseKeyStore
                storePassword = releaseStorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    buildTypes {
        getByName("debug") {
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
        }
        getByName("release") {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
        // Independent installable PR preview: never impersonate the signed
        // production application and never bypass the release signer gate.
        create("preview") {
            initWith(getByName("release"))
            applicationIdSuffix = ".preview"
            versionNameSuffix = "-preview"
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    sourceSets {
        getByName("debug").jniLibs.srcDir("src/debug/jniLibs")
        getByName("release").jniLibs.srcDir("src/release/jniLibs")
        getByName("preview").jniLibs.srcDir("src/preview/jniLibs")
    }

    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }
}

val releaseArtifactTaskName = Regex(
    "(?i)(assemble|bundle|package|install|publish).*release|release.*(bundle|package)",
)

fun isForbiddenArtifactTask(taskName: String): Boolean {
    val lower = taskName.lowercase()
    val debugArtifact = lower.contains("debug") &&
        listOf("assemble", "bundle", "package", "install", "publish", "sign", "finalize")
            .any(lower::startsWith)
    val releaseBundle = lower == "bundlerelease" ||
        lower.endsWith("releasebundle") ||
        lower.endsWith("bundlerelease") ||
        (lower.startsWith("bundle") && lower.endsWith("release"))
    return debugArtifact || releaseBundle
}

fun isTaskExplicitlyExcluded(taskName: String): Boolean {
    return gradle.startParameter.excludedTaskNames.any { excluded ->
        excluded.substringAfterLast(':').equals(taskName, ignoreCase = true)
    }
}

gradle.taskGraph.whenReady(
    org.gradle.api.Action<org.gradle.api.execution.TaskExecutionGraph> {
        val forbiddenTasks = allTasks.filter { task ->
            task.project.path == project.path && isForbiddenArtifactTask(task.name)
        }
        if (forbiddenTasks.isNotEmpty()) {
            throw GradleException(
                "Only signed, versioned production releases or isolated PR preview APKs are supported. Forbidden tasks: " +
                    forbiddenTasks.joinToString { it.path },
            )
        }
        val releaseArtifactRequested = allTasks.any { task ->
            task.project.path == project.path && releaseArtifactTaskName.containsMatchIn(task.name)
        }
        val nativeBuildRequested = allTasks.any { task ->
            task.project.path == project.path && task.name.startsWith("buildRust")
        }
        if (nativeBuildRequested && isTaskExplicitlyExcluded("verifyCargoNdkTool")) {
            throw GradleException(
                "The pinned Rust and cargo-ndk verification gate cannot be excluded.",
            )
        }
        if (releaseArtifactRequested && isTaskExplicitlyExcluded("verifyReleaseApkSigner")) {
            throw GradleException(
                "The release APK signer verification gate cannot be excluded.",
            )
        }
        if (releaseArtifactRequested && !hasReleaseSigning) {
            throw GradleException(
                "Release signing is required. Configure XIANZHUN_RELEASE_STORE_FILE, " +
                    "XIANZHUN_RELEASE_STORE_PASSWORD, XIANZHUN_RELEASE_KEY_ALIAS, and " +
                    "XIANZHUN_RELEASE_KEY_PASSWORD through environment variables.",
            )
        }
        if (releaseArtifactRequested) {
            val actualSigner = configuredReleaseCertificateSha256()
            if (!actualSigner.equals(expectedReleaseSignerSha256, ignoreCase = true)) {
                throw GradleException(
                    "Release certificate mismatch. Expected $expectedReleaseSignerSha256 but found " +
                        actualSigner.lowercase() + ".",
                )
            }
        }
    },
)

configure<AppExtension> {
    applicationVariants.all {
        val apkName = when (buildType.name) {
            "debug" -> "xianzhun-debug-v$appVersionName-debug.apk"
            "release" -> "xianzhun-release-v$appVersionName.apk"
            "preview" -> "xianzhun-preview-v$appVersionName.apk"
            else -> "xianzhun-${buildType.name}-v$appVersionName.apk"
        }
        outputs.all {
            (this as BaseVariantOutputImpl).outputFileName = apkName
        }
    }
}

val verifyCargoNdkTool by tasks.registering {
    group = "verification"
    description = "Verifies the pinned Rust and cargo-ndk tools before native builds."
    doLast {
        verifyPinnedNativeTools()
    }
}

val verifyReleaseApkSigner by tasks.registering {
    group = "verification"
    description = "Verifies the signer embedded in the generated release APK."
    doLast {
        verifyGeneratedReleaseApkSigner()
    }
}

tasks.matching { it.name == "packageRelease" }.configureEach {
    doLast {
        verifyGeneratedReleaseApkSigner()
    }
    finalizedBy(verifyReleaseApkSigner)
}

val buildRustDebug by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds the Rust pitch engine for the debug APK."
    dependsOn(verifyCargoNdkTool)
    doFirst {
        verifyPinnedNativeTools()
        val cargoExecutable = resolveCargoExecutable()
        val rustupExecutable = resolveRustupExecutable()
        val cargoNdkExecutable = resolveCargoNdkExecutable(cargoExecutable)
        val resolvedSdkRoot = requireSdkRoot()
        val resolvedNdkRoot = requireNdkRoot()
        delete(debugJniOutDir)
        debugJniOutDir.mkdirs()
        cargoHome?.let { environment("CARGO_HOME", it.absolutePath) }
        rustupHome?.let { environment("RUSTUP_HOME", it.absolutePath) }
        environment("PATH", toolPath(cargoExecutable, rustupExecutable, cargoNdkExecutable))
        environment("ANDROID_HOME", resolvedSdkRoot.absolutePath)
        environment("ANDROID_SDK_ROOT", resolvedSdkRoot.absolutePath)
        environment("ANDROID_NDK_HOME", resolvedNdkRoot.absolutePath)
        environment("ANDROID_NDK_ROOT", resolvedNdkRoot.absolutePath)
        commandLine(
            cargoExecutable.absolutePath,
            "+$rustToolchain",
            "ndk",
            "-t",
            "arm64-v8a",
            "-t",
            "armeabi-v7a",
            "-t",
            "x86_64",
            "-o",
            debugJniOutDir.absolutePath,
            "-P",
            "26",
            "build",
            "--offline",
        )
    }
    workingDir = rustProjectDir
}

val buildRustRelease by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds the Rust pitch engine for the release APK."
    dependsOn(verifyCargoNdkTool)
    inputs.files(fileTree(rustProjectDir) {
        include("src/**/*.rs", "Cargo.toml", "Cargo.lock", ".cargo/**", "build.rs")
    })
    inputs.property("rustToolchain", requiredRustcVersion)
    inputs.property("cargoNdk", requiredCargoNdkVersion)
    inputs.property("abis", "arm64-v8a,armeabi-v7a,x86_64")
    inputs.property("minimumApi", 26)
    ndkRoot?.resolve("source.properties")?.let { inputs.file(it) }
    outputs.dir(releaseJniOutDir)
    doFirst {
        val cargoExecutable = resolveCargoExecutable()
        val rustupExecutable = resolveRustupExecutable()
        val cargoNdkExecutable = resolveCargoNdkExecutable(cargoExecutable)
        val resolvedSdkRoot = requireSdkRoot()
        val resolvedNdkRoot = requireNdkRoot()
        delete(releaseJniOutDir)
        releaseJniOutDir.mkdirs()
        cargoHome?.let { environment("CARGO_HOME", it.absolutePath) }
        rustupHome?.let { environment("RUSTUP_HOME", it.absolutePath) }
        environment("PATH", toolPath(cargoExecutable, rustupExecutable, cargoNdkExecutable))
        environment("ANDROID_HOME", resolvedSdkRoot.absolutePath)
        environment("ANDROID_SDK_ROOT", resolvedSdkRoot.absolutePath)
        environment("ANDROID_NDK_HOME", resolvedNdkRoot.absolutePath)
        environment("ANDROID_NDK_ROOT", resolvedNdkRoot.absolutePath)
        commandLine(
            cargoExecutable.absolutePath,
            "+$rustToolchain",
            "ndk",
            "-t",
            "arm64-v8a",
            "-t",
            "armeabi-v7a",
            "-t",
            "x86_64",
            "-o",
            releaseJniOutDir.absolutePath,
            "-P",
            "26",
            "build",
            "--release",
            "--offline",
        )
    }
    workingDir = rustProjectDir
}

// A signed preview uses the same optimized Rust code as production, but is
// packaged under com.ofairyo.xianzhun.preview for side-by-side testing.
val buildRustPreview by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds optimized Rust libraries for the signed PR preview APK."
    dependsOn(verifyCargoNdkTool)
    inputs.files(fileTree(rustProjectDir) {
        include("src/**/*.rs", "Cargo.toml", "Cargo.lock", ".cargo/**", "build.rs")
    })
    inputs.property("rustToolchain", requiredRustcVersion)
    inputs.property("cargoNdk", requiredCargoNdkVersion)
    outputs.dir(previewJniOutDir)
    doFirst {
        val cargoExecutable = resolveCargoExecutable()
        val rustupExecutable = resolveRustupExecutable()
        val cargoNdkExecutable = resolveCargoNdkExecutable(cargoExecutable)
        val resolvedSdkRoot = requireSdkRoot()
        val resolvedNdkRoot = requireNdkRoot()
        delete(previewJniOutDir)
        previewJniOutDir.mkdirs()
        cargoHome?.let { environment("CARGO_HOME", it.absolutePath) }
        rustupHome?.let { environment("RUSTUP_HOME", it.absolutePath) }
        environment("PATH", toolPath(cargoExecutable, rustupExecutable, cargoNdkExecutable))
        environment("ANDROID_HOME", resolvedSdkRoot.absolutePath)
        environment("ANDROID_SDK_ROOT", resolvedSdkRoot.absolutePath)
        environment("ANDROID_NDK_HOME", resolvedNdkRoot.absolutePath)
        environment("ANDROID_NDK_ROOT", resolvedNdkRoot.absolutePath)
        commandLine(
            cargoExecutable.absolutePath,
            "+$rustToolchain",
            "ndk",
            "-t", "arm64-v8a",
            "-t", "armeabi-v7a",
            "-t", "x86_64",
            "-o", previewJniOutDir.absolutePath,
            "-P", "26",
            "build", "--release", "--offline",
        )
    }
    workingDir = rustProjectDir
}

tasks.matching { it.name == "mergePreviewJniLibFolders" }.configureEach {
    dependsOn(buildRustPreview)
}

tasks.matching { it.name == "mergeDebugJniLibFolders" }.configureEach {
    dependsOn(buildRustDebug)
}

tasks.matching { it.name == "mergeReleaseJniLibFolders" }.configureEach {
    dependsOn(buildRustRelease)
}
