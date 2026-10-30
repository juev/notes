package org.evsyukov.notes

import android.app.Activity
import android.content.Context
import kotlinx.coroutines.launch
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.UnifiedPush
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage

/**
 * Other devices ask this one to sync at once (S41, S44 in docs/specs/sync.md).
 * The request comes through a UnifiedPush distributor, an app such as ntfy that
 * keeps the one connection to a push server for every app on the device.
 */
object Push {
    /** Whether a distributor was chosen; the address arrives from it a moment later. */
    fun enabled(context: Context) = UnifiedPush.getSavedDistributor(context) != null

    /** Picks the distributor, asking the user when several are installed. `done(false)`: none is. */
    fun enable(activity: Activity, done: (Boolean) -> Unit) {
        turnedOff(activity, false)
        UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { found ->
            if (found) UnifiedPush.register(activity, messageForDistributor = activity.getString(R.string.app_name))
            done(found)
        }
    }

    fun disable(context: Context) {
        turnedOff(context, true)
        UnifiedPush.unregister(context)
        forget(context)
    }

    /** Whether the person turned push off: the distributor confirms that as it confirms a removal of its own. */
    private const val TURNED_OFF = "turnedOff"

    private fun turnedOff(context: Context, value: Boolean) =
        context.getSharedPreferences("push", Context.MODE_PRIVATE).edit().putBoolean(TURNED_OFF, value).apply()

    internal fun turnedOff(context: Context): Boolean =
        context.getSharedPreferences("push", Context.MODE_PRIVATE).getBoolean(TURNED_OFF, false)

    /** Takes the address out of the storage, so that nobody keeps sending to it. */
    internal fun forget(context: Context) {
        Repo.scope.launch {
            runCatching { Repo.store.setPushEndpoint(null) }
            SyncWorker.soon(context)
        }
    }
}

class PushReceiver : PushService() {
    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        Repo.scope.launch {
            runCatching { Repo.store.setPushEndpoint(endpoint.url) }
            // The next run publishes the address for the other devices.
            SyncWorker.soon(this@PushReceiver)
        }
    }

    /** The message says nothing but "sync": whatever it carries is not read. */
    override fun onMessage(message: PushMessage, instance: String) = SyncWorker.now(this)

    /** R95: push is off from here on, and the log says why. */
    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        logError("push", "the distributor did not register the app: $reason")
        Push.forget(this)
    }

    override fun onUnregistered(instance: String) {
        if (!Push.turnedOff(this)) logError("push", "the distributor unregistered the app")
        Push.forget(this)
    }
}
