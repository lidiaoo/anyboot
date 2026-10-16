package com.anyboot.remote

import android.os.Bundle
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
 * 终端设置（独立界面）：服务器模块标题栏那个齿轮图标点进来的。
 *
 * 改动**即时落盘**（没有「保存」按钮，跟本 app 其它地方一致）；已经打开的终端页会
 * 在 onResume 时对齐这份设置，不用重开会话。
 */
class TerminalSettingsActivity : ComponentActivity() {

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
                    BarIcon("‹", "返回") { finish() }
                    Text(
                        "终端设置",
                        fontSize = 16.sp,
                        color = palette.fg,
                        modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                    )
                }

                Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())) {
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

        var dark by remember { mutableStateOf(JumpStore.terminalDark(this@TerminalSettingsActivity)) }
        var fontSize by remember {
            mutableIntStateOf(JumpStore.terminalFontSize(this@TerminalSettingsActivity))
        }
        var autoKeyboard by remember {
            mutableStateOf(JumpStore.terminalAutoKeyboard(this@TerminalSettingsActivity))
        }
        var keyBar by remember {
            mutableStateOf(JumpStore.terminalKeyBar(this@TerminalSettingsActivity))
        }

        // ---- 配色 ----
        SectionTitle("终端配色")
        ThemeRow("暗色（OLED 上省电，长时间看更舒服）", dark) {
            dark = true; JumpStore.setTerminalDark(this@TerminalSettingsActivity, true)
        }
        ThemeRow("亮色", !dark) {
            dark = false; JumpStore.setTerminalDark(this@TerminalSettingsActivity, false)
        }

        // ---- 字号 ----
        SectionTitle("终端字号")
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
        ) {
            ChipButton("−") {
                val next = (fontSize - 1).coerceAtLeast(JumpStore.TERMINAL_FONT_MIN)
                fontSize = next
                JumpStore.setTerminalFontSize(this@TerminalSettingsActivity, next)
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
                JumpStore.setTerminalFontSize(this@TerminalSettingsActivity, next)
            }
            Spacer(Modifier.width(10.dp))
            SecondaryText("范围 ${JumpStore.TERMINAL_FONT_MIN} - ${JumpStore.TERMINAL_FONT_MAX}")
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

        // ---- 行为 ----
        SectionTitle("终端行为")
        CheckRow("进入终端自动弹出键盘", "默认关：先看到输出，要打字时点一下终端区或 ⌨", autoKeyboard) {
            autoKeyboard = it
            JumpStore.setTerminalAutoKeyboard(this@TerminalSettingsActivity, it)
        }
        CheckRow("显示快捷键栏", "终端底部的 Esc / Tab / Ctrl / 方向键 / Enter 那一条", keyBar) {
            keyBar = it
            JumpStore.setTerminalKeyBar(this@TerminalSettingsActivity, it)
        }

        // ---- 恢复默认 ----
        TextButton(
            onClick = {
                dark = true
                fontSize = JumpStore.TERMINAL_FONT_DEFAULT
                autoKeyboard = false
                keyBar = true
                JumpStore.setTerminalDark(this@TerminalSettingsActivity, true)
                JumpStore.setTerminalFontSize(this@TerminalSettingsActivity, JumpStore.TERMINAL_FONT_DEFAULT)
                JumpStore.setTerminalAutoKeyboard(this@TerminalSettingsActivity, false)
                JumpStore.setTerminalKeyBar(this@TerminalSettingsActivity, true)
                toast("已恢复默认：暗色 · 字号 ${JumpStore.TERMINAL_FONT_DEFAULT} · 快捷键栏开")
            },
            modifier = Modifier.align(Alignment.CenterHorizontally),
        ) { Text("恢复默认设置") }

        SecondaryText(
            "改动即时生效：已经打开的终端页返回时会自动套用，也可以直接用终端页顶栏的 "
                    + "A- / A+ 临时调字号（同样会存下来）。",
            Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        )
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
