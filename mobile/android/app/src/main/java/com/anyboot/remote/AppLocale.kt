package com.anyboot.remote

import android.content.Context
import android.content.res.Configuration
import java.util.Locale

/**
 * 应用内语言（A0 基础设施）。
 *
 * 支持 9 种语言（与 PC/桌面端一致）：zh-CN / zh-TW / en-US / ja-JP / ko-KR / de-DE / ru-RU / pt-BR / es-ES。
 * 偏好存 SharedPreferences("anyboot").getString("lang")：空/"auto" = 跟随系统。
 * 各 Activity 在 attachBaseContext 调 [wrap] 即可让本 Activity 生效（切换后 recreate 生效）。
 */
object AppLocale {
    private const val SP = "anyboot"
    private const val KEY = "lang"

    /** 语言代码 -> 显示用本族名 */
    val SUPPORTED: List<Pair<String, String>> = listOf(
        "auto" to "跟随系统 / Auto",
        "zh-CN" to "简体中文",
        "zh-TW" to "繁體中文",
        "en-US" to "English",
        "ja-JP" to "日本語",
        "ko-KR" to "한국어",
        "de-DE" to "Deutsch",
        "ru-RU" to "Русский",
        "pt-BR" to "Português (Brasil)",
        "es-ES" to "Español",
    )

    fun currentTag(ctx: Context): String =
        ctx.getSharedPreferences(SP, Context.MODE_PRIVATE).getString(KEY, "auto") ?: "auto"

    /** 给 HTTP 请求的 Accept-Language 用：auto 时回落到系统语言 */
    fun acceptLanguage(ctx: Context): String {
        val tag = currentTag(ctx)
        return if (tag == "auto" || tag.isBlank()) Locale.getDefault().toLanguageTag() else tag
    }

    fun setLocale(ctx: Context, tag: String) {
        ctx.getSharedPreferences(SP, Context.MODE_PRIVATE).edit().putString(KEY, tag).apply()
        gen++
    }

    /** 语言变更代数：setLocale 时 +1；各 Activity onResume 比对，变了就 recreate（逐页即时切换，无需重启 App）。 */
    private var gen = 0
    private val seen = java.util.WeakHashMap<Any, Int>()
    /** true = 该 Activity 自上次挂载后语言变过，需 recreate */
    fun needsRecreate(host: Any): Boolean {
        val prev = seen[host]
        seen[host] = gen
        return prev != null && prev != gen
    }

    /** 把 base 包成目标语言的 Context（auto 时返回原 base） */
    fun wrap(base: Context): Context {
        val tag = currentTag(base)
        if (tag == "auto" || tag.isBlank()) return base
        val loc = Locale.forLanguageTag(tag)
        Locale.setDefault(loc)
        val config = Configuration(base.resources.configuration)
        config.setLocale(loc)
        config.setLayoutDirection(loc)
        return base.createConfigurationContext(config)
    }
}
