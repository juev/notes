package org.evsyukov.notes.ui

import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.evsyukov.notes.Background
import org.evsyukov.notes.DevicePrefs
import org.evsyukov.notes.Push
import org.evsyukov.notes.R
import org.evsyukov.notes.Repo
import org.evsyukov.notes.Secrets
import org.evsyukov.notes.editor.ColumnWidth
import org.evsyukov.notes.editor.Typography
import uniffi.notes_core.AppSettings
import uniffi.notes_core.Backup
import uniffi.notes_core.BackupSettings
import uniffi.notes_core.ConnectionCheck
import uniffi.notes_core.ExportScope
import uniffi.notes_core.LogLevel
import uniffi.notes_core.StorageBackup
import uniffi.notes_core.StorageBackupStatus
import uniffi.notes_core.SyncConfig
import uniffi.notes_core.SyncSide
import uniffi.notes_core.TextAlign
import uniffi.notes_core.checkSyncConnection
import uniffi.notes_core.codeHighlightAcknowledgements

/** The settings of the app (R80): a page of sections. */
@Composable
fun SettingsScreen(onClose: () -> Unit) {
    BackHandler(onBack = onClose)
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding()) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClose) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
            Text(stringResource(R.string.settings), style = MaterialTheme.typography.titleLarge)
        }
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            General()
            Appearance()
            SyncSection()
            BackupsSection()
            Exchange()
            LogSection()
            About()
        }
    }
}

@Composable
private fun Section(title: Int) {
    HorizontalDivider(Modifier.padding(top = 16.dp))
    Text(stringResource(title), Modifier.padding(vertical = 12.dp), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
}

@Composable
private fun Hint(text: String) {
    Text(text, Modifier.padding(vertical = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** A choice from a list: the title and the current value; a tap lists the others. */
@Composable
private fun <T> Choice(title: Int, current: T, choices: List<Pair<String, T>>, onChoose: (T) -> Unit) {
    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
        Text(stringResource(title), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Picker(choices.firstOrNull { it.second == current }?.first ?: current.toString(), choices, onChoose)
    }
}

@Composable
private fun Toggle(title: Int, on: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(stringResource(title), Modifier.weight(1f))
        Switch(on, onChange, enabled = enabled)
    }
}

/** A setting shared by all devices: it goes to the core and syncs (S12). */
private fun change(update: (AppSettings) -> AppSettings) {
    val new = update(AppModel.settings)
    AppModel.perform<Unit> { it.setSettings(new) }
}

@Composable
private fun General() {
    val context = LocalContext.current
    val settings = AppModel.settings
    Section(R.string.general)
    Choice(
        R.string.theme, DevicePrefs.theme(context),
        listOf(stringResource(R.string.theme_system) to "system", stringResource(R.string.theme_light) to "light", stringResource(R.string.theme_dark) to "dark"),
    ) { DevicePrefs.setTheme(context, it) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(stringResource(R.string.days_in_trash, settings.trashDays.toInt()), Modifier.weight(1f))
        TextButton({ change { it.copy(trashDays = (it.trashDays - 1u).coerceAtLeast(1u)) } }) { Text("−") }
        TextButton({ change { it.copy(trashDays = (it.trashDays + 1u).coerceAtMost(3650u)) } }) { Text("+") }
    }
    val folders = AppModel.folders.map { folder -> AppModel.path(folder.id).joinToString(" / ") { it.name } to folder.id }
    Choice(
        R.string.new_notes_go_to, settings.newNoteFolder,
        listOf(stringResource(R.string.outside_any_folder) to "root", stringResource(R.string.last_folder_used) to "last") + folders,
    ) { value -> change { it.copy(newNoteFolder = value) } }
    Toggle(R.string.smart_typography, settings.smartTypography) { value -> change { it.copy(smartTypography = value) } }
    Toggle(R.string.check_spelling, settings.spellcheck) { value -> change { it.copy(spellcheck = value) } }
    Choice(
        R.string.language, settings.spellLanguage,
        listOf(stringResource(R.string.automatic) to "auto", stringResource(R.string.russian) to "ru", stringResource(R.string.english) to "en"),
    ) { value -> change { it.copy(spellLanguage = value) } }
}

/** E40–E46: what every note looks like. The size and the column belong to this device. */
@Composable
private fun Appearance() {
    val context = LocalContext.current
    val settings = AppModel.settings
    var size by remember { mutableIntStateOf(DevicePrefs.textSize(context)) }
    var column by remember { mutableStateOf(DevicePrefs.columnWidth(context)) }
    Section(R.string.appearance)
    Choice(R.string.text_font, settings.fontBody, Typography.textFonts.map { (if (it == "system") stringResource(R.string.theme_system) else it) to it }) { value ->
        change { it.copy(fontBody = value) }
    }
    Choice(R.string.code_font, settings.fontMono, Typography.codeFonts.map { (if (it == "system") stringResource(R.string.system_monospace) else it) to it }) { value ->
        change { it.copy(fontMono = value) }
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(stringResource(R.string.text_size, size), Modifier.weight(1f))
        fun set(value: Int) {
            size = value.coerceIn(Typography.sizes)
            DevicePrefs.setTextSize(context, size)
        }
        TextButton({ set(size - 1) }) { Text("−") }
        TextButton({ set(size + 1) }) { Text("+") }
    }
    Choice(R.string.line_height, settings.lineHeight, Typography.lineHeights.map { "%.1f".format(it) to it }) { value -> change { it.copy(lineHeight = value) } }
    Choice(R.string.paragraph_spacing, settings.paragraphSpacing, Typography.paragraphSpacings.map { "%.2f".format(it) to it }) { value -> change { it.copy(paragraphSpacing = value) } }
    Choice(
        R.string.column_width, column,
        listOf(
            stringResource(R.string.column_narrow) to ColumnWidth.NARROW, stringResource(R.string.column_medium) to ColumnWidth.MEDIUM,
            stringResource(R.string.column_wide) to ColumnWidth.WIDE, stringResource(R.string.column_full) to ColumnWidth.FULL,
        ),
    ) {
        column = it
        DevicePrefs.setColumnWidth(context, it)
    }
    Choice(R.string.alignment, settings.align, listOf(stringResource(R.string.align_left) to TextAlign.LEFT, stringResource(R.string.align_justified) to TextAlign.JUSTIFY)) { value ->
        change { it.copy(align = value) }
    }
    Toggle(R.string.hyphenation, settings.hyphenate, settings.align != TextAlign.JUSTIFY) { value -> change { it.copy(hyphenate = value) } }
}

private data class Replacement(val config: SyncConfig, val side: SyncSide)

/** The settings of sync (R80–R82, S33, S44): WebDAV, push and the encryption of the storage. */
@Composable
private fun SyncSection() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var enabled by remember { mutableStateOf(false) }
    var saved by remember { mutableStateOf(false) }
    var url by remember { mutableStateOf("") }
    var user by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var passphrase by remember { mutableStateOf("") }
    var pushServer by remember { mutableStateOf("") }
    var pushSignIn by remember { mutableStateOf(false) }
    var pushToken by remember { mutableStateOf("") }
    var pushRefused by remember { mutableStateOf(false) }
    var receiving by remember { mutableStateOf(Push.enabled(context)) }
    var message by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var joining by remember { mutableStateOf<SyncConfig?>(null) }
    var replacing by remember { mutableStateOf<Replacement?>(null) }
    var sealing by remember { mutableStateOf(false) }
    val status = AppModel.syncStatus
    val done = stringResource(R.string.connected)
    val willCreate = stringResource(R.string.connected_will_create)
    val noConnection = stringResource(R.string.no_connection)

    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) {
            val store = Repo.store
            val config = runCatching { store.syncConfig() }.getOrDefault(SyncConfig.Off)
            val server = runCatching { store.pushServer() }.getOrNull().orEmpty()
            val token = Secrets.load(context, Secrets.PUSH_TOKEN).orEmpty()
            val secret = Secrets.load(context, Secrets.PASSWORD).orEmpty()
            val phrase = Secrets.load(context, Secrets.PASSPHRASE).orEmpty()
            withContext(Dispatchers.Main) {
                (config as? SyncConfig.WebDav)?.let {
                    enabled = true
                    saved = true
                    url = it.url
                    user = it.user
                }
                pushServer = server
                pushToken = token
                pushSignIn = token.isNotEmpty()
                password = secret
                passphrase = phrase
            }
        }
    }
    // The subscription is retried in the background, so the answer comes later than the save.
    LaunchedEffect(Unit) {
        while (true) {
            pushRefused = withContext(Dispatchers.IO) { runCatching { Repo.store.pushRefused() }.getOrDefault(false) }
            delay(2000)
        }
    }

    /** The rest of saving, once the storage is stored. */
    fun finish() {
        val store = Repo.store
        val config = if (enabled) SyncConfig.WebDav(url, user) else SyncConfig.Off
        store.setPushServer(if (enabled) pushServer.ifBlank { null } else null)
        val token = if (pushSignIn) pushToken.trim() else ""
        Secrets.save(context, Secrets.PUSH_TOKEN, token)
        store.setPushToken(token.ifEmpty { null })
        if (config is SyncConfig.WebDav) {
            Secrets.save(context, Secrets.PASSWORD, password)
            store.setSyncPassword(password)
            Secrets.save(context, Secrets.PASSPHRASE, passphrase)
            store.setSyncPassphrase(passphrase.ifEmpty { null })
        }
    }

    fun apply(config: SyncConfig, side: SyncSide) {
        busy = true
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    Repo.store.joinStorage(config, side)
                    finish()
                }
            }
            busy = false
            result.onSuccess {
                saved = config !is SyncConfig.Off
                if (saved) Background.askOnce(context)
                AppModel.syncNow()
            }.onFailure { message = describe(it) }
        }
    }

    fun save() {
        val config = if (enabled) SyncConfig.WebDav(url, user) else SyncConfig.Off
        message = null
        busy = true
        scope.launch {
            // S33: a storage that is new to this device may hold data of its own.
            val conflict = withContext(Dispatchers.IO) {
                runCatching {
                    val store = Repo.store
                    config != SyncConfig.Off && store.syncConfig() != config && store.syncConflict(config, password)
                }
            }
            busy = false
            conflict.onSuccess { both -> if (both) joining = config else apply(config, SyncSide.MERGE) }
                .onFailure { message = "$noConnection: ${describe(it)}" }
        }
    }

    fun test() {
        message = null
        busy = true
        val config = SyncConfig.WebDav(url, user)
        scope.launch {
            val result = withContext(Dispatchers.IO) { runCatching { checkSyncConnection(config, password) } }
            busy = false
            message = result.fold({ if (it == ConnectionCheck.READY) done else willCreate }, { "$noConnection: ${describe(it)}" })
        }
    }

    Section(R.string.sync)
    Toggle(R.string.sync_webdav, enabled) { enabled = it }
    if (!enabled) Hint(stringResource(R.string.data_on_this_device))
    if (enabled) {
        OutlinedTextField(url, { url = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.address)) }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri))
        OutlinedTextField(user, { user = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.user_name)) }, singleLine = true)
        OutlinedTextField(password, { password = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.password)) }, singleLine = true, visualTransformation = PasswordVisualTransformation())
        Hint(stringResource(R.string.app_password_hint))
        OutlinedTextField(passphrase, { passphrase = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.storage_passphrase)) }, singleLine = true, visualTransformation = PasswordVisualTransformation())
        Hint(stringResource(R.string.passphrase_hint))
        OutlinedTextField(pushServer, { pushServer = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.push_server)) }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri))
        Hint(stringResource(R.string.push_hint))
        Toggle(R.string.push_sign_in, pushSignIn) { pushSignIn = it }
        if (pushSignIn) {
            OutlinedTextField(pushToken, { pushToken = it }, Modifier.fillMaxWidth(), label = { Text(stringResource(R.string.push_token)) }, singleLine = true, visualTransformation = PasswordVisualTransformation())
        }
        if (pushRefused) Hint(stringResource(R.string.push_refused))
        // S44: receiving goes through a distributor app on the device.
        val activity = context as? android.app.Activity
        Toggle(R.string.push_receive, receiving) { on ->
            if (on && activity != null) {
                Push.enable(activity) { found ->
                    receiving = found
                    if (!found) message = context.getString(R.string.push_no_distributor)
                }
            } else {
                Push.disable(context)
                receiving = false
            }
        }
        Row(Modifier.fillMaxWidth().clickable { Background.open(context) }.padding(vertical = 8.dp)) {
            Text(stringResource(R.string.background_work), Modifier.weight(1f))
            Text(stringResource(if (Background.unrestricted(context)) R.string.background_free else R.string.background_limited), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        Button(::save, enabled = !busy) { Text(stringResource(R.string.save_and_sync)) }
        if (enabled) OutlinedButton(::test, enabled = !busy) { Text(stringResource(R.string.test_connection)) }
        if (busy) CircularProgressIndicator(Modifier.size(20.dp))
    }
    if (saved) {
        TextButton({ sealing = true }) { Text(stringResource(if (status.encrypted) R.string.change_passphrase else R.string.encrypt_storage)) }
    }
    message?.let { Hint(it) }
    status.lastError?.let { Hint(stringResource(R.string.sync_failed, it)) }
    if (status.lastError == null) status.lastOk?.let { Hint(stringResource(R.string.synced_at, it)) }

    joining?.let { config ->
        AlertDialog(
            onDismissRequest = { joining = null },
            title = { Text(stringResource(R.string.both_hold_notes)) },
            text = {
                Column {
                    Text(stringResource(R.string.join_hint))
                    TextButton({
                        joining = null
                        apply(config, SyncSide.MERGE)
                    }) { Text(stringResource(R.string.merge)) }
                    TextButton({
                        joining = null
                        replacing = Replacement(config, SyncSide.STORAGE)
                    }) { Text(stringResource(R.string.use_storage)) }
                    TextButton({
                        joining = null
                        replacing = Replacement(config, SyncSide.DEVICE)
                    }) { Text(stringResource(R.string.use_device)) }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton({ joining = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    replacing?.let { choice ->
        ConfirmDialog(
            stringResource(if (choice.side == SyncSide.STORAGE) R.string.replace_device_text else R.string.replace_storage_text),
            stringResource(R.string.replace),
            { apply(choice.config, choice.side) }, { replacing = null },
        )
    }
    if (sealing) {
        PassphraseDialog(status.encrypted, { sealing = false }) { chosen ->
            sealing = false
            busy = true
            scope.launch {
                val result = withContext(Dispatchers.IO) {
                    runCatching {
                        if (status.encrypted) Repo.store.changeStoragePassphrase(chosen) else Repo.store.encryptStorage(chosen)
                        Secrets.save(context, Secrets.PASSPHRASE, chosen)
                    }
                }
                busy = false
                result.onSuccess {
                    passphrase = chosen
                    AppModel.syncNow()
                }.onFailure { message = describe(it) }
            }
        }
    }
}

/** A new passphrase, typed twice, with the warning that a forgotten one loses the storage. */
@Composable
private fun PassphraseDialog(change: Boolean, onDismiss: () -> Unit, onDone: (String) -> Unit) {
    var first by remember { mutableStateOf("") }
    var second by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(if (change) R.string.change_passphrase else R.string.encrypt_storage)) },
        text = {
            Column {
                Text(stringResource(if (change) R.string.change_passphrase_hint else R.string.encrypt_hint))
                OutlinedTextField(first, { first = it }, label = { Text(stringResource(R.string.passphrase)) }, singleLine = true, visualTransformation = PasswordVisualTransformation())
                OutlinedTextField(second, { second = it }, label = { Text(stringResource(R.string.passphrase_again)) }, singleLine = true, visualTransformation = PasswordVisualTransformation())
                Hint(stringResource(R.string.passphrase_warning))
            }
        },
        confirmButton = { TextButton({ onDone(first) }, enabled = first.isNotEmpty() && first == second) { Text(stringResource(R.string.save)) } },
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** Where a backup to restore comes from. */
private sealed class BackupSource {
    abstract val label: String

    data class Local(val path: String, override val label: String) : BackupSource()
    data class Stored(val copy: StorageBackup, override val label: String) : BackupSource()
}

/** The backups of this device (R70–R72): the schedule, the number to keep, the backups themselves and restoring; the copies in the storage (S32). */
@Composable
private fun BackupsSection() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var settings by remember { mutableStateOf(BackupSettings(24u, 10u)) }
    var backups by remember { mutableStateOf<List<Backup>>(emptyList()) }
    var storage by remember { mutableStateOf<StorageBackupStatus?>(null) }
    var stored by remember { mutableStateOf<List<StorageBackup>>(emptyList()) }
    var restoring by remember { mutableStateOf<BackupSource?>(null) }
    var menu by remember { mutableStateOf<Backup?>(null) }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf<Backup?>(null) }

    fun load() {
        scope.launch {
            withContext(Dispatchers.IO) {
                val store = Repo.store
                settings = runCatching { store.backupSettings() }.getOrDefault(settings)
                backups = runCatching { store.backups() }.getOrDefault(emptyList())
                storage = runCatching { store.storageBackupStatus() }.getOrNull()
                if (storage?.available == true) stored = runCatching { store.storageBackups() }.getOrDefault(emptyList())
            }
        }
    }
    LaunchedEffect(Unit) { load() }

    fun run(done: String? = null, then: () -> Unit = {}, work: (uniffi.notes_core.Store) -> Unit) {
        busy = true
        scope.launch {
            val result = withContext(Dispatchers.IO) { runCatching { work(Repo.store) } }
            busy = false
            message = result.fold({ done }, ::describe)
            if (result.isSuccess) then()
            load()
        }
    }

    fun save(new: BackupSettings) = run { store ->
        store.setBackupSettings(new)
        settings = new
    }

    fun restore(what: BackupSource, overStorage: Boolean) {
        val restored = context.getString(R.string.restored_from, what.label)
        run(restored, { AppModel.syncNow() }) { store ->
            when (what) {
                is BackupSource.Local -> store.restoreBackup(what.path, overStorage)
                is BackupSource.Stored -> store.restoreStorageBackup(what.copy.device, what.copy.name, overStorage)
            }
            Repo.changed()
        }
    }

    val pickFile = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            scope.launch {
                val copy = withContext(Dispatchers.IO) { AttachmentFiles.copyToCache(context, uri) }
                if (copy != null) restoring = BackupSource.Local(copy.absolutePath, copy.name)
            }
        }
    }
    val saveCopy = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/zip")) { uri ->
        val backup = saving
        saving = null
        if (uri != null && backup != null) {
            run(context.getString(R.string.copy_saved)) { _ ->
                context.contentResolver.openOutputStream(uri, "w")?.use { out -> java.io.File(backup.path).inputStream().use { it.copyTo(out) } }
            }
        }
    }

    Section(R.string.backups)
    Choice(
        R.string.create_backups, settings.everyHours,
        listOf(stringResource(R.string.do_not_create) to 0u, stringResource(R.string.daily) to 24u, stringResource(R.string.weekly) to 168u),
    ) { save(settings.copy(everyHours = it)) }
    Choice(R.string.keep_at_most, settings.keep, listOf(5u, 10u, 20u, 30u).map { it.toString() to it }) { save(settings.copy(keep = it)) }
    Hint(stringResource(R.string.backups_hint))
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton({ run { it.createBackup() } }, enabled = !busy) { Text(stringResource(R.string.create_backup_now)) }
        OutlinedButton({ pickFile.launch(arrayOf("*/*")) }, enabled = !busy) { Text(stringResource(R.string.restore_from_file)) }
    }
    message?.let { Hint(it) }
    if (backups.isEmpty()) Hint(stringResource(R.string.no_backups))
    backups.forEach { backup ->
        Row(Modifier.fillMaxWidth().clickable { menu = backup }.padding(vertical = 8.dp)) {
            Text(backup.created.take(16).replace('T', ' '), Modifier.weight(1f))
            Text(android.text.format.Formatter.formatShortFileSize(context, backup.size.toLong()), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    storage?.takeIf { it.available }?.let { status ->
        Toggle(R.string.keep_in_storage, status.enabled) { on -> run { it.setStorageBackups(on) } }
        Hint(stringResource(R.string.storage_backups_hint))
        status.error?.let { Hint(stringResource(R.string.storage_copy_failed, it)) }
        stored.forEach { copy ->
            val label = copy.created.take(16).replace('T', ' ')
            Row(Modifier.fillMaxWidth().clickable { restoring = BackupSource.Stored(copy, label) }.padding(vertical = 8.dp)) {
                Text(label, Modifier.weight(1f))
                Text(stringResource(if (copy.thisDevice) R.string.this_device else R.string.another_device), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
    menu?.let { backup ->
        val label = backup.created.take(16).replace('T', ' ')
        MenuDialog(label, { menu = null }) {
            MenuRow(stringResource(R.string.restore)) {
                menu = null
                restoring = BackupSource.Local(backup.path, label)
            }
            MenuRow(stringResource(R.string.save_a_copy)) {
                menu = null
                saving = backup
                saveCopy.launch(backup.name)
            }
            MenuRow(stringResource(R.string.delete)) {
                menu = null
                run { it.deleteBackup(backup.name) }
            }
        }
    }
    restoring?.let { what ->
        val synced = AppModel.syncStatus.configured
        AlertDialog(
            onDismissRequest = { restoring = null },
            title = { Text(stringResource(R.string.replace_with_backup)) },
            text = {
                Column {
                    Text(stringResource(R.string.restore_text, what.label))
                    // S38: with sync on, what the storage holds has to be settled in the same step.
                    if (synced) {
                        TextButton({
                            restoring = null
                            restore(what, false)
                        }) { Text(stringResource(R.string.restore_merge)) }
                        TextButton({
                            restoring = null
                            restore(what, true)
                        }) { Text(stringResource(R.string.restore_replace)) }
                    } else {
                        TextButton({
                            restoring = null
                            restore(what, false)
                        }) { Text(stringResource(R.string.restore)) }
                    }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton({ restoring = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

/** R60, R62, R65: everything out, and a backup of Standard Notes in. */
@Composable
private fun Exchange() {
    val context = LocalContext.current
    val export = rememberExport()
    var backup by remember { mutableStateOf<Uri?>(null) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { backup = it }
    Section(R.string.import_export)
    OutlinedButton({ pick.launch(arrayOf("*/*")) }) { Text(stringResource(R.string.import_title)) }
    OutlinedButton({ export(ExportJob.Markdown(ExportScope.All)) }) { Text(stringResource(R.string.export_all_markdown)) }
    OutlinedButton({ export(ExportJob.Json(false)) }) { Text(stringResource(R.string.export_json)) }
    OutlinedButton({ export(ExportJob.Json(true)) }) { Text(stringResource(R.string.export_json_files)) }
    backup?.let { chosen ->
        ImportDialog({ files, options -> AppModel.importBackup(context, chosen, files, options) }, { backup = null })
    }
}

/** R95: the level of the log and the way to its files. */
@Composable
private fun LogSection() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var level by remember { mutableStateOf(LogLevel.ERROR) }
    LaunchedEffect(Unit) { level = withContext(Dispatchers.IO) { runCatching { Repo.store.logLevel() }.getOrDefault(LogLevel.ERROR) } }
    Section(R.string.log)
    Choice(
        R.string.level, level,
        listOf(
            stringResource(R.string.log_none) to LogLevel.OFF, stringResource(R.string.log_errors) to LogLevel.ERROR,
            stringResource(R.string.log_events) to LogLevel.INFO, stringResource(R.string.log_detailed) to LogLevel.DEBUG,
        ),
    ) { chosen ->
        level = chosen
        scope.launch(Dispatchers.IO) { runCatching { Repo.store.setLogLevel(chosen) } }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton({ scope.launch { shareLog(context) } }) { Text(stringResource(R.string.share)) }
        OutlinedButton({ scope.launch(Dispatchers.IO) { runCatching { Repo.store.clearLog() } } }) { Text(stringResource(R.string.clear)) }
    }
    Hint(stringResource(R.string.log_hint))
}

/** R95: the files of the log go as copies in the cache: the folder they live in holds the database. */
private suspend fun shareLog(context: android.content.Context) {
    val files = withContext(Dispatchers.IO) {
        val out = java.io.File(context.cacheDir, "shared").apply { mkdirs() }
        runCatching { Repo.store.logFiles() }.getOrDefault(emptyList()).map { path ->
            val copy = java.io.File(path).copyTo(java.io.File(out, java.io.File(path).name), overwrite = true)
            androidx.core.content.FileProvider.getUriForFile(context, "${context.packageName}.files", copy)
        }
    }
    if (files.isEmpty()) return
    val intent = Intent(Intent.ACTION_SEND_MULTIPLE).setType("text/plain")
        .putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(files)).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    runCatching { context.startActivity(Intent.createChooser(intent, null)) }
}

/** E3, E40: what the licenses of the syntaxes and the fonts ask to show. */
@Composable
private fun About() {
    val context = LocalContext.current
    var shown by remember { mutableStateOf<String?>(null) }
    Section(R.string.about)
    val version = remember { runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull() ?: "?" }
    Text("Notes $version")
    TextButton({
        shown = buildString {
            listOf("iawriter", "inter", "jetbrainsmono", "literata").forEach { name ->
                runCatching { context.assets.open("fonts/OFL-$name.txt").bufferedReader().use { append(it.readText()).append("\n\n") } }
            }
            append(codeHighlightAcknowledgements())
        }
    }) { Text(stringResource(R.string.notices)) }
    shown?.let { text -> ImportReportNotice(text) { shown = null } }
}

@Composable
private fun ImportReportNotice(text: String, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        text = { Column(Modifier.verticalScroll(rememberScrollState())) { Text(text, style = MaterialTheme.typography.bodySmall) } },
        confirmButton = { TextButton(onDismiss) { Text(stringResource(R.string.close)) } },
    )
}
