package org.evsyukov.notes

import android.app.Application
import android.content.Context
import android.content.pm.ApplicationInfo
import android.os.Build
import android.os.StrictMode
import androidx.compose.runtime.mutableStateOf
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.evsyukov.notes.editor.ColumnWidth
import org.evsyukov.notes.editor.Typography
import uniffi.notes_core.AppException
import uniffi.notes_core.LogLevel
import uniffi.notes_core.Store
import uniffi.notes_core.SyncConfig
import uniffi.notes_core.SyncReport
import uniffi.notes_core.finishPushes
import java.io.File
import java.util.concurrent.TimeUnit

class NotesApp : Application() {
    /** The database lives in the app's private storage; nothing else on the device can read it. */
    val store: Store by lazy {
        Store.open(File(filesDir, "notes").absolutePath).also {
            fun load(source: String, name: String) = Secrets.load(this, name) { error ->
                it.log(LogLevel.ERROR, source, "the stored $name was not decrypted: ${logged(error)}")
            }
            it.setSyncPassword(load("sync", Secrets.PASSWORD))
            it.setPushToken(load("push", Secrets.PUSH_TOKEN))
            it.setSyncPassphrase(load("sync", Secrets.PASSPHRASE))
            // S71: the other devices name this one in the history of versions.
            runCatching { it.setDeviceName(Build.MODEL) }
                .onFailure { error -> it.log(LogLevel.ERROR, "app", "the name of the device was not set: ${logged(error)}") }
            // R95: where a log of events begins for this run.
            val version = runCatching { packageManager.getPackageInfo(packageName, 0).versionName }.getOrNull() ?: "?"
            it.log(LogLevel.INFO, "app", "started $version on Android ${Build.VERSION.RELEASE}")
        }
    }

    override fun onCreate() {
        super.onCreate()
        instance = this
        // A debug build reports in the system log what reads or writes the disk on the main thread.
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            StrictMode.setThreadPolicy(StrictMode.ThreadPolicy.Builder().detectDiskReads().detectDiskWrites().penaltyLog().build())
        }
        SyncWorker.schedule(this)
        BackupWorker.schedule(this)
        Repo.scope.launch {
            // Opening may back the database up and migrate it; here, not in the first screen that asks.
            runCatching { store }.onFailure { android.util.Log.e("Notes", "the store did not open", it) }
        }
    }

    companion object {
        lateinit var instance: NotesApp
            internal set
    }
}

/**
 * An error as the cause in a line of the log (R95): the text of the core, and of any other error only its class,
 * because the message of an error about a file or an intent names the file.
 */
fun logged(error: Throwable): String = when (error) {
    is AppException.NotFound -> "not found"
    is AppException.Invalid -> "invalid: ${error.msg}"
    is AppException.Storage -> "storage: ${error.msg}"
    is AppException.Sync -> "sync: ${error.msg}"
    is AppException.TooLarge -> "too large: ${error.msg}"
    else -> error.javaClass.simpleName
}

/** R95: an ERROR line of the log. Nothing when the store does not open: the core writes the log through it. */
fun logError(source: String, message: String) {
    runCatching { Repo.store.log(LogLevel.ERROR, source, message) }
}

/** One place every screen goes through to reach the core and to learn that data changed. */
object Repo {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    val store: Store get() = NotesApp.instance.store

    /** The way of the screens to the core: one call at a time, in the order they were made. Sync runs beside it. */
    @OptIn(ExperimentalCoroutinesApi::class)
    val core = Dispatchers.IO.limitedParallelism(1)

    /** Bumped after every change, local or pulled; screens reload when it moves. */
    val revision = MutableStateFlow(0)
    val syncing = MutableStateFlow(false)

    private val syncMutex = Mutex()
    private var moving: Job? = null

    /**
     * S39: call after a local write: refreshes the screens and syncs after two quiet seconds.
     * The sync is handed to the system, so it still happens when the process is gone by then
     * or the network comes back later.
     */
    fun changed() {
        revision.update { it + 1 }
        SyncWorker.soon(NotesApp.instance)
    }

    suspend fun sync(): Result<SyncReport> = withContext(Dispatchers.IO) {
        // R70: the automatic backup is looked at with every run, with sync on or off.
        runCatching { store.backupIfDue() }
        val config = runCatching { store.syncConfig() }
            .onFailure { logError("sync", "the sync settings were not read: ${logged(it)}") }
            .getOrElse { return@withContext Result.failure(it) }
        if (config is SyncConfig.Off) return@withContext Result.success(SyncReport(0u, 0u, 0u, 0u))
        val result = syncMutex.withLock {
            syncing.value = true
            val result = runCatching { store.syncNow() }
            syncing.value = false
            // The status changed either way; data only if something arrived.
            revision.update { it + 1 }
            result
        }
        if (result.isSuccess) moveAttachments()
        // S41: nudges leave after the run; a background worker must not report done, and be stopped, before they have gone.
        finishPushes()
        result
    }

    /**
     * S24: the content of attachments moves after the fields are on screen, and the next run
     * for the fields does not wait for it. One pass at a time; the screens reload when it ends.
     */
    @Synchronized
    private fun moveAttachments() {
        if (moving?.isActive == true) return
        moving = scope.launch {
            runCatching { store.syncAttachments() }
            revision.update { it + 1 }
        }
    }

    /** Waits for the pass over attachment content that is going, if one is. */
    suspend fun attachmentsMoved() {
        moving?.join()
    }
}

/**
 * S39: sync as a system job: every 15 minutes (the platform minimum) to pull what
 * was changed elsewhere while the app is closed, and once after a local write.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val synced = Repo.sync().isSuccess
        // The job holds the process until the content of attachments has moved as well (S24).
        Repo.attachmentsMoved()
        return if (synced) Result.success() else Result.retry()
    }

    companion object {
        private val online = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

        /** One run at once, for a nudge from another device. The system lets it start even from the background. */
        fun now(context: Context) {
            val request = OneTimeWorkRequestBuilder<SyncWorker>()
                .setConstraints(online)
                // Before Android 12 an expedited job needs a foreground notification; a plain one will do there.
                .apply { if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST) }
                .build()
            WorkManager.getInstance(context)
                .enqueueUniqueWork("sync-now", ExistingWorkPolicy.REPLACE, request)
        }

        /** One run two seconds from now; a newer call replaces one still waiting. */
        fun soon(context: Context) {
            val request = OneTimeWorkRequestBuilder<SyncWorker>()
                .setInitialDelay(2, TimeUnit.SECONDS)
                .setConstraints(online)
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
                .build()
            WorkManager.getInstance(context)
                .enqueueUniqueWork("sync-soon", ExistingWorkPolicy.REPLACE, request)
        }

        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES)
                .setConstraints(online)
                .build()
            WorkManager.getInstance(context)
                .enqueueUniquePeriodicWork("sync", ExistingPeriodicWorkPolicy.KEEP, request)
        }
    }
}

/**
 * R70: the automatic backup is looked at once an hour as a system job of its own. It needs no network,
 * unlike the job of sync that looks at it as well, so a device that stays offline still gets its backups.
 */
class BackupWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    // A failure is in the log already (R95); the next hour tries again.
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        runCatching { Repo.store.backupIfDue() }
        Result.success()
    }

    companion object {
        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<BackupWorker>(1, TimeUnit.HOURS).build()
            WorkManager.getInstance(context)
                .enqueueUniquePeriodicWork("backup", ExistingPeriodicWorkPolicy.KEEP, request)
        }
    }
}

/** What this device keeps for itself and does not sync: the look, the size of the text and the like. */
object DevicePrefs {
    private fun prefs(context: Context) = context.getSharedPreferences("device", Context.MODE_PRIVATE)

    /** Set once the look is chosen; the theme reads it, so the screen is repainted at once. */
    private val chosenTheme = mutableStateOf<String?>(null)

    /** R80: `system`, `light` or `dark`. */
    fun theme(context: Context): String = chosenTheme.value ?: prefs(context).getString("theme", null) ?: "system"

    fun setTheme(context: Context, value: String) {
        prefs(context).edit().putString("theme", value).apply()
        chosenTheme.value = value
    }

    /** E41: nothing saved is the body size of the platform. */
    fun textSize(context: Context): Int =
        Typography.size(prefs(context).takeIf { it.contains("textSize") }?.getInt("textSize", BODY_SIZE), BODY_SIZE)

    fun setTextSize(context: Context, value: Int) = prefs(context).edit().putInt("textSize", value).apply()

    /** E44: on this device. */
    fun columnWidth(context: Context): ColumnWidth =
        ColumnWidth.entries.firstOrNull { it.name == prefs(context).getString("columnWidth", null) } ?: ColumnWidth.MEDIUM

    fun setColumnWidth(context: Context, value: ColumnWidth) = prefs(context).edit().putString("columnWidth", value.name).apply()

    /** R12: the folders collapsed in the sidebar. */
    fun foldedFolders(context: Context): Set<String> = prefs(context).getStringSet("foldedFolders", null).orEmpty()

    fun setFoldedFolders(context: Context, value: Set<String>) = prefs(context).edit().putStringSet("foldedFolders", value).apply()

    /** R16: the display of a system view, by the key of the view. */
    fun display(context: Context, key: String): ViewDisplay = ViewDisplay.decoded(prefs(context).getString("display.$key", null))

    fun setDisplay(context: Context, key: String, value: ViewDisplay) =
        prefs(context).edit().putString("display.$key", value.encoded()).apply()

    /** R2: the folder a new note goes to when no folder is open. */
    fun lastFolder(context: Context): String? = prefs(context).getString("lastFolder", null)

    fun setLastFolder(context: Context, id: String) = prefs(context).edit().putString("lastFolder", id).apply()

    private const val BODY_SIZE = 16
}
