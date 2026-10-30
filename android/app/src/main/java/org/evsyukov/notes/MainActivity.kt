package org.evsyukov.notes

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.evsyukov.notes.ui.AppModel
import org.evsyukov.notes.ui.MainScreen
import org.evsyukov.notes.ui.theme.NotesTheme

/** The theme of the app; the icons of the system bars follow the look that is chosen (R80). */
@Composable
fun AppTheme(content: @Composable () -> Unit) {
    val dark = when (DevicePrefs.theme(LocalContext.current)) {
        "light" -> false
        "dark" -> true
        else -> isSystemInDarkTheme()
    }
    val view = LocalView.current
    SideEffect {
        (view.context as? Activity)?.window?.let { window ->
            WindowCompat.getInsetsController(window, view).run {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }
    NotesTheme(dark) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background, content = content)
    }
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { AppTheme { MainScreen() } }
        openFrom(intent)

        // S39: sync when the app comes forward and once a minute while it stays there.
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.RESUMED) {
                while (true) {
                    Repo.sync()
                    delay(60_000)
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        openFrom(intent)
    }

    /** R86: the launcher shortcut and the tile make a new note; a shared note opens. */
    private fun openFrom(intent: Intent?) {
        if (intent?.action == ACTION_NEW_NOTE) {
            intent.action = null
            AppModel.newNote()
        }
        intent?.getStringExtra(EXTRA_NOTE)?.let {
            intent.removeExtra(EXTRA_NOTE)
            AppModel.open(it)
        }
    }

    override fun onStop() {
        super.onStop()
        AppModel.flushEditsNow()
    }

    companion object {
        const val ACTION_NEW_NOTE = "org.evsyukov.notes.NEW_NOTE"
        const val EXTRA_NOTE = "note"
    }
}
