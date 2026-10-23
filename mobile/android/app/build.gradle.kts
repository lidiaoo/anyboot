plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.anyboot.remote"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.anyboot.remote"
        minSdk = 24
        targetSdk = 34
        versionCode = 3
        versionName = "1.2"
    }

    buildTypes {
        getByName("debug") {
            isMinifyEnabled = false
        }
        getByName("release") {
            // 没有签名配置时 release 也能编译（产物是未签名 APK）；
            // 要装到手机上请用 debug 包，或自己配 signingConfig。
            isMinifyEnabled = false
        }
    }

    // 界面层已全部迁移 Jetpack Compose（Material 3），业务逻辑仍是 Java
    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    // SSH/Telnet/SFTP 能力：mwiede 维护的 JSch 分叉（包名同为 com.jcraft.jsch，drop-in），
    // 老版 0.1.55 不支持 rsa-sha2-*，连现代 OpenSSH 会被拒。
    // ⚠️ 不要升到 0.2.27+/2.x：它会改变主机密钥算法协商结果，而本 app 的 TOFU 信任库是
    // **按服务器记一条指纹**（不是按 key type 记），于是老用户已存的指纹对不上 → 直接
    // `JSchUnknownHostKeyException: reject HostKey`。升版等于让所有老装机全部连不上。
    implementation("com.github.mwiede:jsch:0.2.20")

    // ⛔ 不要加 Bouncy Castle（org.bouncycastle:bcprov-jdk18on）。
    // 实测（emulator-5554，跳板机 81.68.251.231:8022）：
    //   · 不加 BC → 连接正常，日志 `会话建立成功`；
    //   · 加上 BC（jsch 仍是 0.2.20）→ 立刻变成 `经由跳板机连接失败：reject HostKey: [81.68.251.231]:8022`。
    // 原因是 BC 到位后 jsch 会多做一套 EdDSA 签名/密钥实现，**主机密钥算法协商结果随之为变**，
    // 于是协商到的 host key 与老装机已存的那条指纹不同 → TofuRepository.check 返回 NOT_INCLUDED
    // → 硬拒绝（本 app 的 UserInfo 不代答确认框，只能去服务器详情里「重置信任」）。
    // 而本 app 的 TOFU 是**按服务器记一条指纹**、不按 key type 记，所以这是一次面向全部老装机的破坏性变更。
    // 想上 BC 必须先改 TOFU（按 key type 存 / 或指纹变了要弹窗让用户重新确认），届时再一并评估。
    //
    // 同理：jsch 也不要升到 0.2.27+/2.x —— 0.2.27 才有 writeOpenSSHv1PrivateKey()（EdDSA 落盘的唯一途径），
    // 但那同样会改变主机密钥协商，触发上面一模一样的老装机全连不上。

    // Jetpack Compose（BOM 统一版本；material-icons-core 由 material3 传递引入）
    val composeBom = platform("androidx.compose:compose-bom:2024.10.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.9.3")

    // 远程接入扫码配对：CameraX 取景 + zxing core 解码。
    // zxing core 是纯解码库（不带相机 UI / 权限代码），与 CameraX 组合是轻量标配；
    // 不引 zxing-embedded（它绑 appcompat 与旧 view 体系，本项目界面已全 Compose）
    val camerax = "1.3.4"
    implementation("androidx.camera:camera-core:$camerax")
    implementation("androidx.camera:camera-camera2:$camerax")
    implementation("androidx.camera:camera-lifecycle:$camerax")
    implementation("androidx.camera:camera-view:$camerax")
    implementation("com.google.zxing:core:3.5.3")
}
