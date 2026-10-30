package org.evsyukov.notes

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings

/**
 * Whether the system lets Notes work in the background freely (S44 in
 * docs/specs/sync.md). An app taken out of battery optimisation is not held
 * back by Doze and by the app standby buckets, so sync by schedule and by a
 * nudge from another device run when they are due.
 */
object Background {
    private const val PREFS = "background"
    private const val ASKED = "asked"

    fun unrestricted(context: Context): Boolean =
        context.getSystemService(PowerManager::class.java)?.isIgnoringBatteryOptimizations(context.packageName) == true

    /** The system dialog that exempts Notes, or the system list when there is nothing left to ask. */
    @SuppressLint("BatteryLife")
    fun open(context: Context) {
        val intent = if (unrestricted(context)) Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
        else Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${context.packageName}"))
        // A phone without such a screen is left as it is.
        runCatching { context.startActivity(intent) }
    }

    /** Asks by itself one time only; what the user answered is theirs to change in the settings. */
    fun askOnce(context: Context) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (unrestricted(context) || prefs.getBoolean(ASKED, false)) return
        prefs.edit().putBoolean(ASKED, true).apply()
        open(context)
    }
}
