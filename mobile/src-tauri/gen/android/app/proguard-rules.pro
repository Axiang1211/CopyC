# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile
# CopyCBridge 的方法由 Rust 透過 JNI 依名稱呼叫（見 src/android.rs），R8 看不到這些呼叫，不能刪除或改名
-keep class top.dreamsworld.copyc.CopyCBridge { *; }
# 給網頁呼叫的 window.CopyCAndroid（MainActivity）
-keepclassmembers class * {
    @android.webkit.JavascriptInterface <methods>;
}
