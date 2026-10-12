plugins {
    id("com.android.application")
}

android {
    namespace = "com.anyboot.remote"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.anyboot.remote"
        minSdk = 24
        targetSdk = 34
        versionCode = 2
        versionName = "1.1"
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

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
