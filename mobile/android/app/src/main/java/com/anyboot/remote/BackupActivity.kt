package com.anyboot.remote

import android.net.Uri
import androidx.compose.ui.res.stringResource
import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.io.ByteArrayOutputStream
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 备份与恢复（docs/backup-ui-design.md §2）：
 * S1 主页（导出 / 多端同步 / 导入）+ S2 WebDAV 配置 + S3 导入预览 + S4 结果报告。
 * S2/S3/S4 以全屏覆盖层实现（对齐 iOS fullScreenCover 的层级语义）。
 */
class BackupActivity : ComponentActivity() {
    override fun onResume() {
        super.onResume()
        if (AppLocale.needsRecreate(this)) { recreate(); return }
    }

    override fun attachBaseContext(newBase: Context) {
        super.attachBaseContext(AppLocale.wrap(newBase))
    }


    override fun onCreate(savedInstanceState: android.os.Bundle?) {
        super.onCreate(savedInstanceState)
        // 必须包 AppTheme：LocalPalette 在树根 provide，缺了会落到默认值
        // paletteOf(Application())，裸 Application 无 baseContext → 取色 NPE 闪退
        setContent {
            AppTheme {
                BackupPageRoot()
            }
        }
    }
}

// ==================================================================
// S1 主页
// ==================================================================

@Composable
private fun BackupPageRoot() {
    val palette = LocalPalette.current
    val context = LocalContext.current
    var overlay by remember { mutableStateOf("") }   // "" / "webdav" / "preview" / "report"
    var drawerOpen by remember { mutableStateOf(false) }   // 左上角 ☰ 呼出抽屉（与电脑/电脑列表页共用）
    var reportJson by remember { mutableStateOf<BackupManager.Report?>(null) }
    var pendingEnvelope by remember { mutableStateOf<org.json.JSONObject?>(null) }
    var pendingRaw by remember { mutableStateOf<ByteArray?>(null) }

    Box(Modifier.fillMaxSize().background(palette.base)) {
        Column(Modifier.fillMaxSize()) {
            TopBar(stringResource(R.string.bk_titleFull), onMenu = { drawerOpen = true })
            Column(
                Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 16.dp)
                    .padding(bottom = 32.dp)
            ) {
                ExportSection()
                Spacer(Modifier.height(16.dp))
                SyncSection(onOpenConfig = { overlay = "webdav" })
                Spacer(Modifier.height(16.dp))
                ImportSection { raw, encrypted ->
                    pendingRaw = raw
                    if (encrypted) {
                        // S5 口令弹窗
                        showPasswordDialog = true
                    } else {
                        openPreview(context, raw, null) { envelope ->
                            pendingEnvelope = envelope
                            overlay = "preview"
                        }
                    }
                }
            }
        }

        // 抽屉侧边栏（电脑/电脑列表/服务器同款，见 Drawer.kt）
        if (drawerOpen) AppDrawerOverlay(DRAWER_BACKUP) { drawerOpen = false }

        when (overlay) {
            "webdav" -> WebDavOverlay(onClose = { overlay = "" })
            "preview" -> pendingEnvelope?.let { envelope ->
                ImportPreviewOverlay(
                    envelope = envelope,
                    onClose = { overlay = "" },
                    onReport = { report ->
                        reportJson = report
                        overlay = "report"
                    },
                )
            }
            "report" -> reportJson?.let { report ->
                ReportOverlay(report = report, onClose = { overlay = "" })
            }
        }
    }

    // S5 加密口令弹窗
    if (showPasswordDialog) {
        PasswordDialog(
            title = stringResource(R.string.and_backupPwdTitle),
            onConfirm = { password ->
                val raw = pendingRaw
                if (raw == null) {
                    toast(context, context.getString(R.string.and_fileInvalidReselect))
                    showPasswordDialog = false
                    return@PasswordDialog ""
                }
                val error = tryOrMessage {
                    val envelope = BackupManager.decrypt(raw, password)
                    showPasswordDialog = false
                    pendingEnvelope = envelope
                    overlay = "preview"
                }
                error ?: ""
            },
            onDismiss = { showPasswordDialog = false },
        )
    }
}

// S5 弹窗显隐（activity 级状态）
private var showPasswordDialog by mutableStateOf(false)

// ==================================================================
// 通用件
// ==================================================================

@Composable
private fun TopBar(title: String, onMenu: (() -> Unit)? = null) {
    val palette = LocalPalette.current
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = if (onMenu != null) 4.dp else 16.dp, end = 12.dp)
            .height(48.dp),
    ) {
        // 左上角 ☰：呼出抽屉侧边栏（电脑/电脑列表页同款）
        if (onMenu != null) BarIcon("☰", stringResource(R.string.topbar_sidebar), tint = palette.fg, onClick = onMenu)
        Text(
            title,
            fontSize = 16.sp,
            fontWeight = FontWeight.Bold,
            color = palette.fg,
        )
    }
}

@Composable
private fun BkSectionTitle(text: String) {
    val palette = LocalPalette.current
    Text(
        text,
        fontSize = 13.sp,
        color = palette.fgDim,
        modifier = Modifier.padding(top = 16.dp, bottom = 8.dp),
    )
}

@Composable
private fun Card(content: @Composable () -> Unit) {
    val palette = LocalPalette.current
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(palette.card)
            .padding(12.dp)
    ) {
        content()
    }
}

private fun toast(context: android.content.Context, message: String) {
    android.widget.Toast.makeText(context, message, android.widget.Toast.LENGTH_LONG).show()
}

/** try 里做完成功返回 null；异常返回其文案 */
private inline fun tryOrMessage(block: () -> Unit): String? {
    return try {
        block()
        null
    } catch (error: Exception) {
        error.message ?: error.javaClass.simpleName
    }
}

private fun stampName(): String {
    return SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date())
}

// ==================================================================
// 导出区（L1-L3）
// ==================================================================

@Composable
private fun ExportSection() {
    val palette = LocalPalette.current
    val context = LocalContext.current
    val categories = remember { mutableStateListOf("servers", "jumps", "terminal") }
    var password by remember { mutableStateOf("") }
    var passwordAgain by remember { mutableStateOf("") }
    var showPassword by remember { mutableStateOf(false) }
    var exporting by remember { mutableStateOf(false) }
    var exportBytes by remember { mutableStateOf<ByteArray?>(null) }

    fun countOf(cat: String): Int = when (cat) {
        "servers" -> ServerRepository.servers.size
        "jumps" -> JumpStore.loadJumps(context).size
        "keys" -> JumpStore.loadKeys(context).size
        "terminal" -> 1
        "history" -> JumpStore.loadHistory(context).size
        else -> 0
    }

    val keysSelected = categories.contains("keys")
    val passwordValid = password.length >= 6 && password == passwordAgain
    val exportDisabled = categories.isEmpty() || (keysSelected && !passwordValid) || exporting

    val saveLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument("application/octet-stream")
    ) { uri: Uri? ->
        val bytes = exportBytes
        if (uri != null && bytes != null) {
            exporting = true
            Thread({
                val error = tryOrMessage {
                    context.contentResolver.openOutputStream(uri)?.use { out ->
                        out.write(bytes)
                    }
                }
                exporting = false
                val skippedNote = if (categories.contains("keys")) "" else ""
                toast(context, if (error == null) {
                    context.getString(R.string.and_exportedN, categories.size, skippedNote)
                } else context.getString(R.string.and_writeFail, error))
            }, "anyboot-export").start()
        }
        exportBytes = null
    }

    BkSectionTitle(stringResource(R.string.and_tabExport))
    Card {
        BackupManager.CATEGORIES.forEachIndexed { index, cat ->
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable {
                        if (categories.contains(cat)) categories.remove(cat) else categories.add(cat)
                    }
                    .padding(vertical = 6.dp),
            ) {
                Checkbox(
                    checked = categories.contains(cat),
                    onCheckedChange = {
                        if (it) categories.add(cat) else categories.remove(cat)
                    },
                    colors = CheckboxDefaults.colors(checkedColor = palette.accent),
                )
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(BackupManager.labelOf(cat), fontSize = 14.sp, color = palette.fg)
                        if (cat == "keys") {
                            Spacer(Modifier.width(6.dp))
                            Text(stringResource(R.string.and_highRisk), fontSize = 11.sp, color = palette.warn,
                                fontWeight = FontWeight.SemiBold)
                        }
                    }
                    Text(
                        when (cat) {
                            "servers" -> stringResource(R.string.and_catServers)
                            "jumps" -> stringResource(R.string.and_catJump)
                            "keys" -> stringResource(R.string.and_catKeys)
                            "terminal" -> stringResource(R.string.and_catPrefs)
                            else -> stringResource(R.string.and_catTrust)
                        },
                        fontSize = 11.sp,
                        color = palette.fgFaint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Text(stringResource(R.string.and_countParen, countOf(cat)), fontSize = 12.sp, color = palette.fgDim)
            }
        }
        // info / warning 条
        val jumpIds = JumpStore.loadJumps(context).map { it.id }.toSet()
        // 一址一跳板机：每个地址的绑定各算一处（同一台电脑两三个地址挂不同跳板机时，
        // 少了哪台都要在导出前提醒到）
        val missing = ServerStore.load(context).sumOf { profile ->
            val refs = LinkedHashSet<String>()
            if (profile.jumpServerId.isNotEmpty()) refs.add(profile.jumpServerId)
            profile.jumpBindings().forEach { binding ->
                if (binding.size > 1 && binding[1].isNotEmpty()) refs.add(binding[1])
            }
            refs.count { !jumpIds.contains(it) }
        }
        if (keysSelected && !passwordValid) {
            InfoBar(stringResource(R.string.and_mustSetPwd), warn = true)
        } else if (missing > 0) {
            InfoBar(stringResource(R.string.and_missJump, missing), warn = false)
        }
        if (keysSelected) {
            Spacer(Modifier.height(8.dp))
            PasswordFields(
                password = password,
                passwordAgain = passwordAgain,
                showPassword = showPassword,
                onPassword = { password = it },
                onAgain = { passwordAgain = it },
                onToggle = { showPassword = !showPassword },
                mismatch = passwordAgain.isNotEmpty() && password != passwordAgain,
            )
        }
        Spacer(Modifier.height(12.dp))
        PrimaryButton(
            text = if (exporting) stringResource(R.string.and_exporting) else stringResource(R.string.and_exportToFile),
            enabled = !exportDisabled,
            loading = exporting,
        ) {
            try {
                val cats = categories.toSet()
                val pwd = if (cats.contains("keys")) password else null
                val envelope = BackupManager.collect(context, cats, null)
                exportBytes = BackupManager.serialize(envelope, pwd)
                saveLauncher.launch("anyboot-backup-${stampName()}.anyboot")
            } catch (error: Exception) {
                toast(context, context.getString(R.string.and_exportFailed, error.message ?: context.getString(R.string.gw_unknownErr)))
            }
        }
    }
}

@Composable
private fun InfoBar(text: String, warn: Boolean) {
    val palette = LocalPalette.current
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 8.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (warn) palette.warn.copy(alpha = 0.12f) else palette.elevate)
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Text(
            (if (warn) "⚠ " else "ℹ ") + text,
            fontSize = 12.sp,
            color = if (warn) palette.warn else palette.fgDim,
        )
    }
}

@Composable
private fun PasswordFields(
    password: String,
    passwordAgain: String,
    showPassword: Boolean,
    onPassword: (String) -> Unit,
    onAgain: (String) -> Unit,
    onToggle: () -> Unit,
    mismatch: Boolean,
) {
    val palette = LocalPalette.current
    val colors = OutlinedTextFieldDefaults.colors(
        focusedBorderColor = palette.accent,
        unfocusedBorderColor = if (mismatch) palette.bad else palette.lineStrong,
        focusedTextColor = palette.fg,
        unfocusedTextColor = palette.fg,
        cursorColor = palette.accent,
    )
    OutlinedTextField(
        value = password,
        onValueChange = onPassword,
        label = { Text(stringResource(R.string.and_pwdHint), fontSize = 12.sp, color = palette.fgDim) },
        singleLine = true,
        visualTransformation = if (showPassword) VisualTransformation.None
        else PasswordVisualTransformation(),
        trailingIcon = {
            TextButton(onClick = onToggle) {
                Text(if (showPassword) stringResource(R.string.and_hide) else stringResource(R.string.and_show), fontSize = 12.sp, color = palette.accent)
            }
        },
        colors = colors,
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(
        value = passwordAgain,
        onValueChange = onAgain,
        label = { Text(stringResource(R.string.and_pwdAgain), fontSize = 12.sp, color = palette.fgDim) },
        singleLine = true,
        visualTransformation = if (showPassword) VisualTransformation.None
        else PasswordVisualTransformation(),
        colors = colors,
        modifier = Modifier.fillMaxWidth(),
    )
    if (mismatch) {
        Text(stringResource(R.string.bk_pwMismatch), fontSize = 12.sp, color = palette.bad,
            modifier = Modifier.padding(top = 4.dp))
    }
}

@Composable
private fun PrimaryButton(text: String, enabled: Boolean, loading: Boolean = false,
                          danger: Boolean = false, modifier: Modifier = Modifier,
                          onClick: () -> Unit) {
    val palette = LocalPalette.current
    val bg = when {
        !enabled -> palette.elevate
        danger -> palette.bad
        else -> palette.accent
    }
    val fg = if (!enabled) palette.fgFaint else palette.onAccent
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
        modifier = modifier
            .clip(RoundedCornerShape(12.dp))
            .background(bg)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(vertical = 14.dp),
    ) {
        if (loading) {
            CircularProgressIndicator(
                color = fg,
                strokeWidth = 2.dp,
                modifier = Modifier.size(16.dp),
            )
            Spacer(Modifier.width(8.dp))
        }
        Text(text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = fg)
    }
}

// ==================================================================
// 多端同步区（L4-L6）
// ==================================================================

@Composable
private fun SyncSection(onOpenConfig: () -> Unit) {
    val palette = LocalPalette.current
    val context = LocalContext.current
    val prefs = remember { ServerStore.prefs(context) }
    var enabled by remember { mutableStateOf(prefs.getBoolean("backup_sync_enabled", false)) }
    var confirmOff by remember { mutableStateOf(false) }
    var syncing by remember { mutableStateOf(false) }
    val config = remember { SyncManager.Config.load(context) }
    var lastSync by remember { mutableStateOf(prefs.getLong("backup_sync_last_at", 0L)) }

    BkSectionTitle(stringResource(R.string.and_syncTitle))
    Card {
        // L4 启用开关
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.and_enableSync), fontSize = 14.sp, color = palette.fg)
                Text(stringResource(R.string.and_syncDesc),
                    fontSize = 12.sp, color = palette.fgFaint)
            }
            Switch(
                checked = enabled,
                onCheckedChange = { want ->
                    when {
                        !want -> confirmOff = true
                        !config.isConfigured() -> {
                            onOpenConfig()
                            toast(context, context.getString(R.string.and_cfgFirst))
                        }
                        else -> {
                            enabled = true
                            prefs.edit().putBoolean("backup_sync_enabled", true).apply()
                        }
                    }
                },
                colors = SwitchDefaults.colors(checkedTrackColor = palette.accent),
            )
        }
        // L5 配置行
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clickable { onOpenConfig() }
                .padding(vertical = 12.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.and_webdavTitle), fontSize = 14.sp, color = palette.fg)
                Text(
                    if (config.isConfigured())
                        Uri.parse(config.serverUrl.trim()).host ?: config.serverUrl
                    else stringResource(R.string.and_notConfigured),
                    fontSize = 12.sp, color = palette.fgFaint,
                )
            }
            if (config.isConfigured()) {
                Box(
                    Modifier.size(7.dp).clip(CircleShape).background(palette.ok)
                )
            }
        }
        // L6 立即同步行
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clickable(enabled = config.isConfigured() && !syncing) {
                    syncing = true
                    SyncManager.syncNow(context) { error ->
                        syncing = false
                        lastSync = if (error == null) System.currentTimeMillis() else lastSync
                        toast(context, if (error == null) context.getString(R.string.bk_syncDone) else context.getString(R.string.and_syncFail, error))
                    }
                }
                .padding(vertical = 12.dp),
        ) {
            Text(
                stringResource(R.string.bk_syncNow),
                fontSize = 14.sp,
                color = if (config.isConfigured()) palette.fg else palette.fgFaint,
                modifier = Modifier.weight(1f),
            )
            if (syncing) {
                CircularProgressIndicator(
                    color = palette.accent,
                    strokeWidth = 2.dp,
                    modifier = Modifier.size(14.dp),
                )
                Spacer(Modifier.width(6.dp))
                Text(stringResource(R.string.and_syncing), fontSize = 12.sp, color = palette.fgDim)
            } else {
                Text(
                    when {
                        lastSync > 0 -> stringResource(R.string.bk_lastSync, SimpleDateFormat("HH:mm", Locale.US).format(Date(lastSync)))
                        else -> stringResource(R.string.bk_neverSynced)
                    },
                    fontSize = 12.sp,
                    color = palette.fgDim,
                )
            }
        }
    }

    if (confirmOff) {
        AlertDialog(
            onDismissRequest = { confirmOff = false },
            title = { Text(stringResource(R.string.and_offTitle)) },
            text = { Text(stringResource(R.string.and_offDesc)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmOff = false
                    enabled = false
                    prefs.edit().putBoolean("backup_sync_enabled", false).apply()
                }) { Text(stringResource(R.string.and_offBtn), color = LocalPalette.current.bad) }
            },
            dismissButton = {
                TextButton(onClick = { confirmOff = false }) { Text(stringResource(R.string.common_cancel)) }
            },
        )
    }
}

// ==================================================================
// 导入区（L7）+ S5 口令弹窗
// ==================================================================

private fun openPreview(context: android.content.Context, raw: ByteArray,
                        password: String?, onEnvelope: (org.json.JSONObject) -> Unit) {
    Thread({
        try {
            val envelope = if (password == null) BackupManager.parse(raw)
            else BackupManager.decrypt(raw, password)
            android.os.Handler(android.os.Looper.getMainLooper()).post { onEnvelope(envelope) }
        } catch (error: Exception) {
            val message = error.message ?: context.getString(R.string.and_invalidFile)
            android.os.Handler(android.os.Looper.getMainLooper()).post {
                toast(context, message)
            }
        }
    }, "anyboot-import").start()
}

@Composable
private fun ImportSection(onPicked: (ByteArray, Boolean) -> Unit) {
    val palette = LocalPalette.current
    val context = LocalContext.current

    val openLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocument()
    ) { uri: Uri? ->
        if (uri != null) {
            try {
                context.contentResolver.openInputStream(uri)?.use { stream ->
                    val buffer = ByteArrayOutputStream()
                    val chunk = ByteArray(8192)
                    var count: Int
                    while (stream.read(chunk).also { count = it } > 0) {
                        buffer.write(chunk, 0, count)
                        if (buffer.size() > BackupManager.MAX_FILE_BYTES) {
                            throw BackupManager.BackupException(context.getString(R.string.and_tooBig))
                        }
                    }
                    val raw = buffer.toByteArray()
                    onPicked(raw, BackupManager.isEncrypted(raw))
                }
            } catch (error: Exception) {
                toast(context, error.message ?: context.getString(R.string.and_readFileErr))
            }
        }
    }

    BkSectionTitle(stringResource(R.string.and_tabImport))
    Card {
        SecondaryButton(stringResource(R.string.and_importFromFile)) {
            openLauncher.launch(arrayOf("*/*"))
        }
        Spacer(Modifier.height(6.dp))
        Text(
            stringResource(R.string.and_importHint),
            fontSize = 12.sp,
            color = palette.fgFaint,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun SecondaryButton(text: String, enabled: Boolean = true,
                            modifier: Modifier = Modifier, onClick: () -> Unit) {
    val palette = LocalPalette.current
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
        modifier = modifier
            .clip(RoundedCornerShape(12.dp))
            .background(Color.Transparent)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(vertical = 14.dp),
    ) {
        Text(
            text,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (enabled) palette.accent else palette.fgFaint,
        )
    }
}

/** S5 备份口令弹窗；onConfirm 返回错误文案（空 = 成功） */
@Composable
private fun PasswordDialog(
    title: String,
    onConfirm: (String) -> String,
    onDismiss: () -> Unit,
) {
    var password by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    var attempts by remember { mutableStateOf(0) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                OutlinedTextField(
                    value = password,
                    onValueChange = { password = it },
                    label = { Text(stringResource(R.string.and_pwd)) },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    modifier = Modifier.fillMaxWidth(),
                )
                if (error.isNotEmpty()) {
                    Text(
                        error + if (attempts >= 3) stringResource(R.string.and_confirmSource) else "",
                        fontSize = 12.sp,
                        color = LocalPalette.current.bad,
                        modifier = Modifier.padding(top = 6.dp),
                    )
                }
            }
        },
        confirmButton = {
            TextButton(
                enabled = password.isNotEmpty(),
                onClick = {
                    val message = onConfirm(password)
                    if (message.isNotEmpty()) {
                        attempts += 1
                        error = message
                    }
                },
            ) { Text(stringResource(R.string.and_confirmBtn)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.common_cancel)) }
        },
    )
}

// ==================================================================
// S2 WebDAV 配置（全屏覆盖层）
// ==================================================================

@Composable
private fun WebDavOverlay(onClose: () -> Unit) {
    val palette = LocalPalette.current
    val context = LocalContext.current
    val prefs = remember { ServerStore.prefs(context) }
    val saved = remember { SyncManager.Config.load(context) }

    var serverUrl by remember { mutableStateOf(saved.serverUrl) }
    var username by remember { mutableStateOf(saved.username) }
    var password by remember { mutableStateOf(saved.password(context)) }
    var directory by remember { mutableStateOf(saved.directory) }
    var periodMinutes by remember { mutableIntStateOf(saved.periodMinutes) }
    var syncOnChange by remember { mutableStateOf(saved.syncOnChange) }
    val categories = remember {
        mutableStateListOf<String>().apply {
            if (saved.categories.isEmpty()) {
                addAll(listOf("servers", "jumps", "keys", "terminal"))
            } else {
                addAll(saved.categories)
            }
        }
    }
    var encrypt by remember { mutableStateOf(saved.encrypt) }
    var encryptPassword by remember { mutableStateOf(saved.encryptPassword(context)) }
    var encryptAgain by remember { mutableStateOf(encryptPassword) }
    var testing by remember { mutableStateOf(false) }
    var testMessage by remember { mutableStateOf("") }
    var testOk by remember { mutableStateOf(false) }
    var confirmCreate by remember { mutableStateOf(false) }

    val formValid = serverUrl.trim().startsWith("http")
    val encryptValid = !encrypt || (encryptPassword.length >= 6 && encryptPassword == encryptAgain)

    Box(Modifier.fillMaxSize().background(palette.base)) {
        Column(Modifier.fillMaxSize()) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp)
                    .height(52.dp),
            ) {
                Text("‹ " + stringResource(R.string.and_back), fontSize = 14.sp, color = palette.fgDim,
                    modifier = Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .clickable { onClose() }
                        .padding(8.dp))
                Text(stringResource(R.string.and_webdavTitle), fontSize = 16.sp, fontWeight = FontWeight.Bold,
                    color = palette.fg, modifier = Modifier.weight(1f))
                Text(
                    stringResource(R.string.common_save),
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = if (formValid && encryptValid) palette.accent else palette.fgFaint,
                    modifier = Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .clickable(enabled = formValid && encryptValid) {
                            val config = SyncManager.Config.load(context)
                            config.serverUrl = serverUrl.trim()
                            config.username = username.trim()
                            if (password.isNotEmpty()) config.passwordRef =
                                config.savePassword(context, password)
                            config.directory = directory.trim().ifEmpty { "AnyBoot" }
                            config.periodMinutes = periodMinutes
                            config.syncOnChange = syncOnChange
                            config.categories = categories.toSet()
                            config.encrypt = encrypt
                            if (encrypt) config.encryptPasswordRef =
                                config.saveEncryptPassword(context, encryptPassword)
                            config.save(context)
                            prefs.edit().putBoolean("backup_sync_enabled", true).apply()
                            toast(context, context.getString(R.string.and_saved))
                            onClose()
                        }
                        .padding(8.dp),
                )
            }
            Column(
                Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .imePadding()
                    .padding(horizontal = 16.dp)
                    .padding(bottom = 24.dp)
            ) {
                Card {
                    BkFieldLabel(stringResource(R.string.and_serverAddr))
                    FieldInput("https://dav.jianguoyun.com/dav/", serverUrl,
                        { serverUrl = it }, KeyboardType.Uri)
                    BkFieldLabel(stringResource(R.string.server_kUser))
                    FieldInput(stringResource(R.string.and_accountEmail), username, { username = it }, KeyboardType.Email)
                    BkFieldLabel(stringResource(R.string.and_password))
                    FieldInput(stringResource(R.string.and_webdavPwdPh), password, { password = it },
                        KeyboardType.Password)
                    BkFieldLabel(stringResource(R.string.and_remoteSyncDir))
                    FieldInput("AnyBoot", directory, { directory = it }, KeyboardType.Uri)
                    BkFieldLabel(stringResource(R.string.and_syncPeriod))
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        listOf(0 to stringResource(R.string.and_manual), 15 to stringResource(R.string.and_minN, 15), 30 to stringResource(R.string.and_minN, 30), 60 to stringResource(R.string.and_minN, 60))
                            .forEach { (minutes, label) ->
                                val selected = periodMinutes == minutes
                                Text(
                                    label,
                                    fontSize = 12.sp,
                                    fontWeight = if (selected) FontWeight.SemiBold else null,
                                    color = if (selected) palette.accent else palette.fgDim,
                                    modifier = Modifier
                                        .clip(RoundedCornerShape(8.dp))
                                        .background(if (selected) palette.accent.copy(alpha = 0.15f)
                                            else palette.elevate)
                                        .clickable { periodMinutes = minutes }
                                        .padding(horizontal = 10.dp, vertical = 8.dp),
                                )
                            }
                    }
                    Spacer(Modifier.height(10.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(stringResource(R.string.and_syncOnChange), fontSize = 14.sp, color = palette.fg)
                            Text(stringResource(R.string.and_syncOnChangeHint),
                                fontSize = 12.sp, color = palette.fgFaint)
                        }
                        Switch(
                            checked = syncOnChange,
                            onCheckedChange = { syncOnChange = it },
                            colors = SwitchDefaults.colors(checkedTrackColor = palette.accent),
                        )
                    }
                    if (testMessage.isNotEmpty()) {
                        Text(
                            (if (testOk) "✓ " else "✕ ") + testMessage,
                            fontSize = 12.sp,
                            color = if (testOk) palette.ok else palette.bad,
                            modifier = Modifier.padding(top = 8.dp),
                        )
                    }
                }
                Spacer(Modifier.height(12.dp))
                Card {
                    BkFieldLabel(stringResource(R.string.and_syncDataTypes))
                    BackupManager.CATEGORIES.forEach { cat ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    if (categories.contains(cat)) {
                                        categories.remove(cat)
                                    } else {
                                        categories.add(cat)
                                    }
                                },
                        ) {
                            Checkbox(
                                checked = categories.contains(cat),
                                onCheckedChange = {
                                    if (it) categories.add(cat) else categories.remove(cat)
                                },
                                colors = CheckboxDefaults.colors(checkedColor = palette.accent),
                            )
                            Text(BackupManager.labelOf(cat), fontSize = 14.sp, color = palette.fg)
                        }
                    }
                    if (categories.contains("keys") && !encrypt) {
                        InfoBar(stringResource(R.string.and_encryptWarn), warn = true)
                    }
                }
                Spacer(Modifier.height(12.dp))
                Card {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.and_syncFileEncrypt), fontSize = 14.sp, color = palette.fg,
                            modifier = Modifier.weight(1f))
                        Switch(
                            checked = encrypt,
                            onCheckedChange = {
                                encrypt = it
                                if (!it) {
                                    encryptPassword = ""
                                    encryptAgain = ""
                                }
                            },
                            colors = SwitchDefaults.colors(checkedTrackColor = palette.accent),
                        )
                    }
                    if (encrypt) {
                        BkFieldLabel(stringResource(R.string.and_syncPwdHint))
                        FieldInput(stringResource(R.string.and_syncPwdPh), encryptPassword, { encryptPassword = it },
                            KeyboardType.Password)
                        FieldInput(stringResource(R.string.and_syncPwdAgainPh), encryptAgain, { encryptAgain = it },
                            KeyboardType.Password)
                        if (encryptAgain.isNotEmpty() && encryptPassword != encryptAgain) {
                            Text(stringResource(R.string.bk_pwMismatch), fontSize = 12.sp, color = palette.bad)
                        }
                    }
                }
                Spacer(Modifier.height(16.dp))
                // L9 测试连接
                SecondaryButton(if (testing) stringResource(R.string.and_testing) else stringResource(R.string.server_test)) {
                    if (!formValid) return@SecondaryButton
                    testing = true
                    testMessage = ""
                    val probe = SyncManager.Config.load(context)
                    probe.serverUrl = serverUrl.trim()
                    probe.username = username.trim()
                    probe.directory = directory.trim()
                    probe.passwordRef = saved.passwordRef
                    SyncManager.testConnection(context, probe) { status, error ->
                        testing = false
                        when (status) {
                            "ok" -> { testOk = true; testMessage = context.getString(R.string.and_connOk) }
                            "notFound" -> confirmCreate = true
                            else -> { testOk = false; testMessage = error ?: context.getString(R.string.and_connFail) }
                        }
                    }
                }
            }
        }
    }

    if (confirmCreate) {
        AlertDialog(
            onDismissRequest = { confirmCreate = false },
            title = { Text(stringResource(R.string.and_dirNotExist)) },
            text = { Text(stringResource(R.string.and_dirNotExistCreate)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmCreate = false
                    testing = true
                    val probe = SyncManager.Config.load(context)
                    probe.serverUrl = serverUrl.trim()
                    probe.username = username.trim()
                    probe.directory = directory.trim()
                    probe.passwordRef = saved.passwordRef
                    SyncManager.createDirectory(context, probe) { error ->
                        testing = false
                        testOk = error == null
                        testMessage = if (error == null) context.getString(R.string.and_dirCreated) else error
                    }
                }) { Text(stringResource(R.string.fb_create)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmCreate = false }) { Text(stringResource(R.string.common_cancel)) }
            },
        )
    }
}

@Composable
private fun BkFieldLabel(text: String) {
    val palette = LocalPalette.current
    Text(text, fontSize = 12.sp, color = palette.fgDim,
        modifier = Modifier.padding(top = 10.dp, bottom = 2.dp))
}

@Composable
private fun FieldInput(placeholder: String, value: String, onChange: (String) -> Unit,
                       keyboardType: KeyboardType) {
    val palette = LocalPalette.current
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        placeholder = { Text(placeholder, fontSize = 13.sp, color = palette.fgFaint) },
        singleLine = true,
        visualTransformation = if (keyboardType == KeyboardType.Password)
            PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
            keyboardType = keyboardType,
        ),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = palette.accent,
            unfocusedBorderColor = palette.lineStrong,
            focusedTextColor = palette.fg,
            unfocusedTextColor = palette.fg,
            cursorColor = palette.accent,
        ),
        modifier = Modifier.fillMaxWidth(),
    )
}

// ==================================================================
// S3 导入预览（全屏覆盖层）
// ==================================================================

@Composable
private fun ImportPreviewOverlay(
    envelope: org.json.JSONObject,
    onClose: () -> Unit,
    onReport: (BackupManager.Report) -> Unit,
) {
    val palette = LocalPalette.current
    val context = LocalContext.current
    val summary = remember { BackupManager.summarize(context, envelope) }
    val rows = summary.optJSONObject("rows") ?: org.json.JSONObject()
    val presentCats = remember {
        BackupManager.CATEGORIES.filter { rows.has(it) }
    }
    val selected = remember {
        mutableStateListOf<String>().apply { addAll(presentCats) }
    }
    var strategy by remember {
        mutableStateOf(
            ServerStore.prefs(context).getString("backup_import_strategy", "")
                ?.takeIf { it == "overwrite" } ?: "skipExisting")
    }
    var importing by remember { mutableStateOf(false) }
    var confirmOverwrite by remember { mutableStateOf(false) }

    Box(Modifier.fillMaxSize().background(palette.base)) {
        Column(Modifier.fillMaxSize()) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp)
                    .height(52.dp),
            ) {
                Text("‹ " + stringResource(R.string.common_cancel), fontSize = 14.sp, color = palette.fgDim,
                    modifier = Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .clickable(enabled = !importing) { onClose() }
                        .padding(8.dp))
                Text(stringResource(R.string.and_importPreview), fontSize = 16.sp, fontWeight = FontWeight.Bold,
                    color = palette.fg)
            }
            Column(
                Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 16.dp)
                    .padding(bottom = 140.dp)
            ) {
                // 来源行
                Card {
                    Text(stringResource(R.string.and_fromPlatform, platformName(LocalContext.current, summary.optString("platform", ""))),
                        fontSize = 13.sp, fontWeight = FontWeight.Medium, color = palette.fg)
                    Text(
                        BackupManager.isoNow().let { now ->
                            val exported = summary.optString("exportedAt", "")
                            (if (exported.isEmpty()) now else exported) +
                                    stringResource(R.string.and_anybootVer, summary.optString("app", "?"))
                        },
                        fontSize = 12.sp, color = palette.fgDim,
                    )
                }
                Spacer(Modifier.height(12.dp))
                // 分类清单
                Card {
                    presentCats.forEach { cat ->
                        val row = rows.optJSONObject(cat)
                        if (row != null) {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        if (selected.contains(cat)) {
                                            selected.remove(cat)
                                        } else {
                                            selected.add(cat)
                                        }
                                    },
                            ) {
                                Checkbox(
                                    checked = selected.contains(cat),
                                    onCheckedChange = {
                                        if (it) selected.add(cat) else selected.remove(cat)
                                    },
                                    colors = CheckboxDefaults.colors(checkedColor = palette.accent),
                                )
                                Text(BackupManager.labelOf(cat), fontSize = 14.sp,
                                    color = palette.fg, modifier = Modifier.weight(1f))
                                Text(
                                    stringResource(R.string.and_backupLocalCount, row.optInt("backup"), row.optInt("local")),
                                    fontSize = 12.sp, color = palette.fgDim,
                                )
                            }
                            if (cat == "terminal") {
                                Text(stringResource(R.string.and_willOverwrite), fontSize = 11.sp, color = palette.fgFaint,
                                    modifier = Modifier.padding(start = 40.dp, bottom = 6.dp))
                            }
                        }
                    }
                }
                Spacer(Modifier.height(12.dp))
                // 合并策略
                Card {
                    BkFieldLabel(stringResource(R.string.and_mergeStrategy))
                    Row(verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.fillMaxWidth().clickable {
                            strategy = "skipExisting"
                            ServerStore.prefs(context).edit()
                                .putString("backup_import_strategy", "skipExisting").apply()
                        }) {
                        RadioButton(selected = strategy == "skipExisting",
                            onClick = {
                                strategy = "skipExisting"
                                ServerStore.prefs(context).edit()
                                    .putString("backup_import_strategy", "skipExisting").apply()
                            },
                            colors = RadioButtonDefaults.colors(selectedColor = palette.accent))
                        Text(stringResource(R.string.and_skipExisting), fontSize = 14.sp, color = palette.fg)
                    }
                    Row(verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.fillMaxWidth().clickable {
                            strategy = "overwrite"
                            ServerStore.prefs(context).edit()
                                .putString("backup_import_strategy", "overwrite").apply()
                        }) {
                        RadioButton(selected = strategy == "overwrite",
                            onClick = {
                                strategy = "overwrite"
                                ServerStore.prefs(context).edit()
                                    .putString("backup_import_strategy", "overwrite").apply()
                            },
                            colors = RadioButtonDefaults.colors(selectedColor = palette.accent))
                        Text(stringResource(R.string.and_backupOverwrite), fontSize = 14.sp, color = palette.fg)
                    }
                    if (strategy == "overwrite") {
                        Text(stringResource(R.string.and_replaceWarn), fontSize = 12.sp, color = palette.bad)
                    }
                }
                // 警示条
                val missing = summary.optInt("missingJumpRefs", 0)
                if (missing > 0) {
                    Spacer(Modifier.height(12.dp))
                    InfoBar(stringResource(R.string.and_missJumpNotInBackup, missing), warn = true)
                }
            }
        }
        // 底部固定双钮
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .background(palette.card)
                .navigationBarsPadding()
                .padding(16.dp)
        ) {
            if (importing) {
                Text(stringResource(R.string.and_importingNow), fontSize = 12.sp, color = palette.fgDim)
                LinearProgressIndicator(
                    color = palette.accent,
                    modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                SecondaryButton(stringResource(R.string.common_cancel), onClick = onClose,
                    enabled = !importing, modifier = Modifier.weight(1f))
                PrimaryButton(
                    text = stringResource(R.string.and_importSelectedN, selected.size),
                    enabled = selected.isNotEmpty() && !importing,
                    loading = importing,
                    onClick = {
                        if (strategy == "overwrite") {
                            confirmOverwrite = true
                        } else {
                            importing = true
                            runImport(context, envelope, selected.toSet(), strategy) { report ->
                                importing = false
                                onReport(report)
                            }
                        }
                    },
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }

    if (confirmOverwrite) {
        AlertDialog(
            onDismissRequest = { confirmOverwrite = false },
            title = { Text(stringResource(R.string.and_overwriteImport)) },
            text = { Text(stringResource(R.string.and_overwriteImportDesc)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmOverwrite = false
                    importing = true
                    runImport(context, envelope, selected.toSet(), strategy) { report ->
                        importing = false
                        onReport(report)
                    }
                }) { Text(stringResource(R.string.and_overwriteImport), color = LocalPalette.current.bad) }
            },
            dismissButton = {
                TextButton(onClick = { confirmOverwrite = false }) { Text(stringResource(R.string.common_cancel)) }
            },
        )
    }
}

private fun runImport(context: android.content.Context, envelope: org.json.JSONObject,
                      categories: Set<String>, strategy: String,
                      onDone: (BackupManager.Report) -> Unit) {
    Thread({
        try {
            val report = BackupManager.apply(context, envelope, categories, strategy,
                emptySet())
            android.os.Handler(android.os.Looper.getMainLooper()).post { onDone(report) }
        } catch (error: Exception) {
            android.os.Handler(android.os.Looper.getMainLooper()).post {
                toast(context, context.getString(R.string.and_importFailMsg, error.message ?: context.getString(R.string.gw_unknownErr)))
            }
        }
    }, "anyboot-apply").start()
}

private fun platformName(ctx: android.content.Context, raw: String): String = when (raw) {
    "ios" -> "iOS"
    "android" -> "Android"
    "desktop", "macos", "windows", "linux" -> ctx.getString(R.string.and_desktopPlatform)
    else -> if (raw.isEmpty()) ctx.getString(R.string.and_unknownDevice) else raw
}

// ==================================================================
// S4 结果报告（全屏覆盖层）
// ==================================================================

@Composable
private fun ReportOverlay(report: BackupManager.Report, onClose: () -> Unit) {
    val palette = LocalPalette.current
    val allFailed = report.totalFailed()
    val anyImported = report.totalAdded() + report.totalUpdated() > 0
    val (icon, color, title) = when {
        allFailed == 0 -> Triple("✓", palette.ok, stringResource(R.string.and_importDone))
        anyImported -> Triple("⚠", palette.warn, "导入完成（有失败项）")
        else -> Triple("✕", palette.bad, "导入失败")
    }

    Box(Modifier.fillMaxSize().background(palette.base)) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(24.dp)
        ) {
            Spacer(Modifier.height(48.dp))
            Box(
                Modifier.size(88.dp).clip(CircleShape).background(color.copy(alpha = 0.12f)),
                contentAlignment = Alignment.Center,
            ) {
                Text(icon, fontSize = 40.sp, fontWeight = FontWeight.Bold, color = color)
            }
            Text(title, fontSize = 20.sp, fontWeight = FontWeight.Bold, color = palette.fg,
                modifier = Modifier.padding(top = 16.dp))
            // 摘要行
            val lines = mutableListOf<String>()
            listOf(
                "servers" to report.servers,
                "jumps" to report.jumps,
                "keys" to report.keys,
                "terminal" to report.terminal,
                "history" to report.history,
            ).forEach { (cat, row) ->
                if (row != null) {
                    val parts = mutableListOf<String>()
                    if (row.optInt("added") > 0) parts.add(stringResource(R.string.and_addedCount, row.optInt("added")))
                    if (row.optInt("updated") > 0) parts.add(stringResource(R.string.and_overwrittenCount, row.optInt("updated")))
                    if (row.optInt("skipped") > 0) parts.add(stringResource(R.string.and_skippedCount, row.optInt("skipped")))
                    if (parts.isNotEmpty()) {
                        lines.add("${BackupManager.labelOf(cat)}：${parts.joinToString(" · ")}")
                    }
                }
            }
            if (allFailed > 0) lines.add(stringResource(R.string.and_nFailsParen, allFailed))
            Text(
                if (lines.isEmpty()) stringResource(R.string.and_noImportableContent) else lines.joinToString("\n"),
                fontSize = 13.sp, color = palette.fgDim,
                modifier = Modifier.padding(top = 8.dp),
            )
            if (allFailed > 0) {
                Text(stringResource(R.string.and_failNotAffectImported), fontSize = 11.sp, color = palette.fgFaint,
                    modifier = Modifier.padding(top = 4.dp))
                Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(top = 16.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(palette.card)
                        .padding(12.dp)
                ) {
                    listOf(report.servers, report.jumps, report.keys, report.terminal,
                        report.history).forEach { row ->
                        val failed = row?.optJSONArray("failed") ?: return@forEach
                        for (i in 0 until failed.length()) {
                            Row(Modifier.padding(vertical = 2.dp)) {
                                Text("✕ ", fontSize = 12.sp, color = palette.bad,
                                    fontWeight = FontWeight.Bold)
                                Text(failed.optString(i), fontSize = 12.sp, color = palette.fg)
                            }
                        }
                    }
                }
            }
            Spacer(Modifier.weight(1f))
            PrimaryButton(if (anyImported) stringResource(R.string.and_doneBtn) else stringResource(R.string.and_back), enabled = true, onClick = onClose)
        }
    }
}
