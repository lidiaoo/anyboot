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
    implementation("com.github.mwiede:jsch:0.2.20")

    // Jetpack Compose（BOM 统一版本；material-icons-core 由 material3 传递引入）
    val composeBom = platform("androidx.compose:compose-bom:2024.10.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.9.3")
}
