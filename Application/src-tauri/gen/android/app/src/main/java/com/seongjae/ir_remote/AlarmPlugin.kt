package com.seongjae.ir_remote

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * The bridge between the Rust core and Android's AlarmManager.
 *
 * Rust owns the alarm list and writes it to `alarms.json`; it passes that path
 * here rather than letting Kotlin guess, because `app_config_dir()` resolves
 * through Tauri's own path plugin and is not something this side can recompute.
 */
@InvokeArg
class SyncArgs {
    lateinit var alarmsPath: String
    lateinit var baseUrl: String
    var commandTimeoutMs: Int = 3000
}

@TauriPlugin
class AlarmPlugin(private val activity: Activity) : Plugin(activity) {

    /**
     * Point the receiver at the current alarms file and device, then re-arm
     * every enabled alarm.
     *
     * Returns whether the system will honour EXACT alarms. When false the
     * alarms are still scheduled, but the OS may batch them by minutes - the UI
     * says so rather than promising a time it cannot keep.
     */
    @Command
    fun sync(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SyncArgs::class.java)
            AlarmStore.save(activity, args.alarmsPath, args.baseUrl, args.commandTimeoutMs)
            AlarmScheduler.syncAll(activity)

            val result = JSObject()
            result.put("exact", AlarmScheduler.canScheduleExact(activity))
            invoke.resolve(result)
        } catch (e: Exception) {
            invoke.reject("could not sync alarms: ${e.message}")
        }
    }

    /** Whether exact alarms are currently permitted. */
    @Command
    fun canScheduleExact(invoke: Invoke) {
        val result = JSObject()
        result.put("exact", AlarmScheduler.canScheduleExact(activity))
        invoke.resolve(result)
    }

    /**
     * Open the system screen where the user grants exact-alarm permission.
     *
     * Android 12+ only, and it cannot be granted from inside the app - the user
     * has to toggle it. Without it alarms drift instead of firing on the minute.
     */
    @Command
    fun requestExactPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
            !AlarmScheduler.canScheduleExact(activity)
        ) {
            try {
                activity.startActivity(
                    Intent(
                        Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM,
                        Uri.parse("package:${activity.packageName}"),
                    )
                )
            } catch (e: Exception) {
                invoke.reject("could not open the exact-alarm setting: ${e.message}")
                return
            }
        }
        invoke.resolve()
    }
}
