package top.dreamsworld.copyc

import android.app.Activity
import androidx.activity.result.ActivityResult
import androidx.activity.result.IntentSenderRequest
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.android.gms.auth.api.identity.AuthorizationRequest
import com.google.android.gms.auth.api.identity.AuthorizationResult
import com.google.android.gms.auth.api.identity.Identity
import com.google.android.gms.common.api.Scope

@InvokeArg
class AuthorizeArgs {
  /** false：只在背景換新的 token，需要使用者同意時直接回報，不跳出畫面 */
  var interactive: Boolean = true
}

/**
 * Google 授權（Android 原生）：用系統的帳號選擇與同意畫面取得 access token，
 * 權限和電腦版一樣只有 openid、email 與 drive.appdata（CopyC 在 Drive 的隱藏資料夾）。
 * Google Cloud 的「Android」用戶端用套件名稱 + 簽章指紋辨認這個 App，App 裡不需要放用戶端 ID。
 */
@TauriPlugin
class GoogleAuthPlugin(private val activity: Activity) : Plugin(activity) {
  private val scopes = listOf(
    Scope("openid"),
    Scope("email"),
    Scope("https://www.googleapis.com/auth/drive.appdata"),
  )

  @Command
  fun authorize(invoke: Invoke) {
    val args = invoke.parseArgs(AuthorizeArgs::class.java)
    val request = AuthorizationRequest.builder().setRequestedScopes(scopes).build()
    Identity.getAuthorizationClient(activity).authorize(request)
      .addOnSuccessListener { result ->
        val pending = result.pendingIntent
        if (result.hasResolution() && pending != null) {
          if (!args.interactive) {
            invoke.reject("consent required", "consent")
          } else {
            startIntentSenderForResult(invoke, IntentSenderRequest.Builder(pending.intentSender).build(), "onAuthorized")
          }
        } else {
          resolveToken(invoke, result)
        }
      }
      .addOnFailureListener { e -> invoke.reject(e.message ?: "authorization failed", "failed") }
  }

  @ActivityCallback
  private fun onAuthorized(invoke: Invoke, result: ActivityResult) {
    if (result.resultCode != Activity.RESULT_OK) {
      invoke.reject("cancelled", "cancelled")
      return
    }
    try {
      resolveToken(invoke, Identity.getAuthorizationClient(activity).getAuthorizationResultFromIntent(result.data))
    } catch (e: Exception) {
      invoke.reject(e.message ?: "authorization failed", "failed")
    }
  }

  private fun resolveToken(invoke: Invoke, result: AuthorizationResult) {
    val token = result.accessToken
    if (token.isNullOrEmpty()) {
      invoke.reject("no access token", "failed")
      return
    }
    val out = JSObject()
    out.put("accessToken", token)
    invoke.resolve(out)
  }
}
