package com.anyboot.remote

import android.content.Context
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/**
 * 语义色板：取值沿用 [Ui] 的 token（对齐 docs/client-ui/mobile-redesign-all.html 定稿），
 * 深色为默认，支持亮色。主题模式三挡 auto/dark/light 的判定也走 [Ui.dark]。
 */
data class Palette(
    val base: Color,
    val card: Color,
    val elevate: Color,
    val line: Color,
    val lineStrong: Color,
    val fg: Color,
    val fgDim: Color,
    val fgFaint: Color,
    val ok: Color,
    val onAccent: Color,
    val accent: Color,
    val warn: Color,
    val bad: Color,
)

fun paletteOf(context: Context): Palette = Palette(
    base = Color(Ui.base(context)),
    card = Color(Ui.card(context)),
    elevate = Color(Ui.elevate(context)),
    line = Color(Ui.line(context)),
    lineStrong = Color(Ui.lineStrong(context)),
    fg = Color(Ui.fg(context)),
    fgDim = Color(Ui.fgDim(context)),
    fgFaint = Color(Ui.fgFaint(context)),
    ok = Color(Ui.ok(context)),
    onAccent = Color(Ui.onAccent(context)),
    accent = Color(Ui.accent(context)),
    warn = Color(Ui.warn(context)),
    bad = Color(Ui.bad(context)),
)

/** Compose 树根部 provide 一次，全树取色都从这走 */
val LocalPalette = staticCompositionLocalOf {
    paletteOf(android.app.Application())
}

/** 终端区底色：终端惯例恒深，不随主题翻面 */
val TerminalDarkBg = Color(0xFF04070F)
val TerminalLightBg = Color(0xFFFFFFFF)
