package org.evsyukov.notes

import android.app.Application
import android.content.pm.ApplicationInfo
import android.os.StrictMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import uniffi.notes_core.Store
import java.io.File

class NotesApp : Application() {
    /** The database lives in the app's private storage; nothing else on the device can read it. */
    val store: Store by lazy { Store.open(File(filesDir, "notes").absolutePath) }

    override fun onCreate() {
        super.onCreate()
        instance = this
        // A debug build reports in the system log what reads or writes the disk on the main thread.
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            StrictMode.setThreadPolicy(StrictMode.ThreadPolicy.Builder().detectDiskReads().detectDiskWrites().penaltyLog().build())
        }
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

/** One place every screen goes through to reach the core and to learn that data changed. */
object Repo {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    val store: Store get() = NotesApp.instance.store
}
