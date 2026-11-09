import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    compileSdk = 37
    namespace = "top.dreamsworld.copyc"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "top.dreamsworld.copyc"
        minSdk = 26
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    // 上傳金鑰（upload key）：CI 從 GitHub Secrets 解出 keystore 後用環境變數指定；
    // 沒有設定時正式版暫用 debug 簽章，方便自己安裝測試（不能上架，也不能直接覆蓋安裝正式簽章的版本）
    val keystorePath = System.getenv("COPYC_ANDROID_KEYSTORE")
    val hasKeystore = keystorePath != null && file(keystorePath).exists()
    signingConfigs {
        create("release") {
            if (hasKeystore) {
                storeFile = file(keystorePath!!)
                storePassword = System.getenv("COPYC_ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("COPYC_ANDROID_KEY_ALIAS") ?: "copyc"
                keyPassword = System.getenv("COPYC_ANDROID_KEYSTORE_PASSWORD")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.getByName(if (hasKeystore) "release" else "debug")
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    // Google 授權（帳號選擇與同意畫面，取得 Drive App 專用資料夾的 access token）
    implementation("com.google.android.gms:play-services-auth:21.3.0")
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
