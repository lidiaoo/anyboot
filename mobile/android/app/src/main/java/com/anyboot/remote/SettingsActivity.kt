package com.anyboot.remote

import android.os.Bundle
import androidx.compose.ui.res.stringResource
import android.content.Context
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * 设置（独立界面，侧边栏「设置」进入）：**语言** + **主题** + **终端设置** 三区块。
 * 原服务器模块标题栏的齿轮入口已合并到这里（齿轮按钮已删除）。
 * 改动即时落盘（无「保存」按钮）；已开终端页 onResume 时对齐。
 */
class SettingsActivity : ComponentActivity() {
    override fun attachBaseContext(newBase: Context) {
        super.attachBaseContext(AppLocale.wrap(newBase))
    }


    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        setContent { Root() }
    }

    @Composable
    private fun Root() {
        // ⚠️ 取色必须在 AppTheme 里面：LocalPalette 的默认值是 paletteOf(Application())，
        // 那个 Application 没有 base context，一旦读到就是 ServerStore.prefs 上的 NPE。
        AppTheme {
            val palette = LocalPalette.current
            BackHandler(enabled = true) { finish() }

            Column(Modifier.fillMaxSize().background(palette.base)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.card)
                        .padding(8.dp),
                ) {
                    BarIcon("‹", stringResource(R.string.and_back)) { finish() }
                    Text(
                        stringResource(R.string.and_settings),
                        fontSize = 16.sp,
                        color = palette.fg,
                        modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                    )
                }

                Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())) {
                    LanguageSection()
                    ThemeSection()
                    SettingsBody()
                    Spacer(Modifier.height(24.dp))
                }
            }
        }
    }

    /** 内容整体挂在调用方的 Column 里（要用 align 居中，所以声明成 ColumnScope 扩展） */
    @Composable
    private fun ColumnScope.SettingsBody() {
        val palette = LocalPalette.current

        var dark by remember { mutableStateOf(JumpStore.terminalDark(this@SettingsActivity)) }
        var fontSize by remember {
            mutableIntStateOf(JumpStore.terminalFontSize(this@SettingsActivity))
        }
        var autoKeyboard by remember {
            mutableStateOf(JumpStore.terminalAutoKeyboard(this@SettingsActivity))
        }
        var keyBar by remember {
            mutableStateOf(JumpStore.terminalKeyBar(this@SettingsActivity))
        }

        // ---- 配色 ----
        SectionTitle(stringResource(R.string.and_termColor))
        ThemeRow(stringResource(R.string.and_termThemeDark), dark) {
            dark = true; JumpStore.setTerminalDark(this@SettingsActivity, true)
        }
        ThemeRow(stringResource(R.string.and_termThemeLight), !dark) {
            dark = false; JumpStore.setTerminalDark(this@SettingsActivity, false)
        }

        // ---- 字号 ----
        SectionTitle(stringResource(R.string.and_termFontSize))
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
        ) {
            ChipButton("−") {
                val next = (fontSize - 1).coerceAtLeast(JumpStore.TERMINAL_FONT_MIN)
                fontSize = next
                JumpStore.setTerminalFontSize(this@SettingsActivity, next)
            }
            Text(
                "$fontSize",
                fontSize = 15.sp,
                color = palette.fg,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
            ChipButton("＋") {
                val next = (fontSize + 1).coerceAtMost(JumpStore.TERMINAL_FONT_MAX)
                fontSize = next
                JumpStore.setTerminalFontSize(this@SettingsActivity, next)
            }
            Spacer(Modifier.width(10.dp))
            SecondaryText(stringResource(R.string.and_termFontRange, JumpStore.TERMINAL_FONT_MIN, JumpStore.TERMINAL_FONT_MAX))
        }
        // 按真实配色 + 真实字号给一行样例，省得来回切页面看效果
        val sampleBg = if (dark) TerminalDarkBg else TerminalLightBg
        val sampleFg = if (dark) Color(0xFFD8E0F0) else Color(0xFF1A1A1A)
        Box(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 2.dp)
                .clip(RoundedCornerShape(8.dp))
                .background(sampleBg)
                .padding(horizontal = 12.dp, vertical = 10.dp),
        ) {
            Text(
                "root@anyboot:~# uname -a",
                fontFamily = FontFamily.Monospace,
                fontSize = fontSize.sp,
                color = sampleFg,
            )
        }

        // ---- 字体族 ----
        // xterm 的 CSS font-family：空 = 资产页内置 Menlo/等宽栈；字体缺失由
        // WebView 自动回落等宽，不会白屏 —— 所以不做存在性检查，改动即时落盘。
        SectionTitle(stringResource(R.string.and_termFontFamily))
        var fontFamily by remember {
            mutableStateOf(JumpStore.terminalFontFamily(this@SettingsActivity))
        }
        OutlinedTextField(
            value = fontFamily,
            onValueChange = {
                fontFamily = it
                JumpStore.setTerminalFontFamily(this@SettingsActivity, it)
            },
            singleLine = true,
            placeholder = { Text(stringResource(R.string.and_termFontPh), fontSize = 12.sp) },
            supportingText = {
                Text(
                    "填 CSS font-family，如 JetBrains Mono / Menlo；缺失自动回落等宽。返回终端页即生效。",
                    fontSize = 11.sp,
                )
            },
            shape = RoundedCornerShape(12.dp),
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp),
        )

        // ---- 行为 ----
        SectionTitle(stringResource(R.string.and_termBehavior))
        CheckRow(stringResource(R.string.and_termAutoKeyboard), stringResource(R.string.and_termAutoKeyboardDesc), autoKeyboard) {
            autoKeyboard = it
            JumpStore.setTerminalAutoKeyboard(this@SettingsActivity, it)
        }
        CheckRow(stringResource(R.string.and_termKeyBar), stringResource(R.string.and_termKeyBarDesc), keyBar) {
            keyBar = it
            JumpStore.setTerminalKeyBar(this@SettingsActivity, it)
        }

        // ---- 恢复默认 ----
        TextButton(
            onClick = {
                dark = true
                fontSize = JumpStore.TERMINAL_FONT_DEFAULT
                fontFamily = ""
                autoKeyboard = false
                keyBar = true
                JumpStore.setTerminalDark(this@SettingsActivity, true)
                JumpStore.setTerminalFontSize(this@SettingsActivity, JumpStore.TERMINAL_FONT_DEFAULT)
                JumpStore.setTerminalFontFamily(this@SettingsActivity, "")
                JumpStore.setTerminalAutoKeyboard(this@SettingsActivity, false)
                JumpStore.setTerminalKeyBar(this@SettingsActivity, true)
                toast(getString(R.string.and_termResetDone, JumpStore.TERMINAL_FONT_DEFAULT))
            },
            modifier = Modifier.align(Alignment.CenterHorizontally),
        ) { Text(stringResource(R.string.and_termResetBtn)) }

        SecondaryText(
            stringResource(R.string.and_termLiveHint),
            Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }

    @Composable
    private fun LanguageSection() {
        val palette = LocalPalette.current
        val ctx = this@SettingsActivity
        SectionTitle(stringResource(R.string.and_language))
        AppLocale.SUPPORTED.forEach { (tag, name) ->
            val selected = AppLocale.currentTag(ctx) == tag
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable {
                        AppLocale.setLocale(ctx, tag)
                        recreate()
                    }
                    .padding(horizontal = 12.dp, vertical = 9.dp),
            ) {
                Text(name, fontSize = 14.sp, color = if (selected) palette.accent else palette.fg, modifier = Modifier.weight(1f))
                if (selected) Text("✓", fontSize = 14.sp, color = palette.accent)
            }
        }
    }

    @Composable
    private fun ThemeSection() {
        val palette = LocalPalette.current
        val ctx = this@SettingsActivity
        SectionTitle(stringResource(R.string.and_themeMode))
        val options = listOf(
            Ui.MODE_AUTO to stringResource(R.string.and_themeAuto),
            Ui.MODE_DARK to stringResource(R.string.and_themeDark),
            Ui.MODE_LIGHT to stringResource(R.string.and_themeLight),
        )
        val current = Ui.mode(ctx)
        options.forEach { (mode, label) ->
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { Ui.setMode(ctx, mode); recreate() }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
            ) {
                RadioButton(selected = current == mode, onClick = { Ui.setMode(ctx, mode); recreate() })
                Text(label, fontSize = 14.sp, color = palette.fg, modifier = Modifier.weight(1f))
            }
        }
    }

    @Composable
    private fun ThemeRow(label: String, selected: Boolean, onSelect: () -> Unit) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onSelect)
                .padding(horizontal = 8.dp, vertical = 2.dp),
        ) {
            RadioButton(selected = selected, onClick = onSelect)
            Text(label, fontSize = 14.sp, color = LocalPalette.current.fg, modifier = Modifier.weight(1f))
        }
    }

    @Composable
    private fun CheckRow(title: String, hint: String, checked: Boolean, onChecked: (Boolean) -> Unit) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clickable { onChecked(!checked) }
                .padding(horizontal = 8.dp, vertical = 2.dp),
        ) {
            Checkbox(checked = checked, onCheckedChange = onChecked)
            Column(Modifier.weight(1f).padding(end = 12.dp)) {
                Text(title, fontSize = 14.sp, color = LocalPalette.current.fg)
                SecondaryText(hint)
            }
        }
    }
}
