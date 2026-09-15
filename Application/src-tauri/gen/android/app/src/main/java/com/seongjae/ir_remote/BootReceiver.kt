package com.seongjae.ir_remote

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Re-arms alarms after a reboot.
 *
 * AlarmManager forgets everything when the device restarts, so without this an
 * alarm set on Monday would stop firing the moment the phone was rebooted —
 * silently, which is the worst way for it to fail.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED,
            "android.intent.action.QUICKBOOT_POWERON" -> {
                Log.i("BootReceiver", "re-arming alarms after ${intent.action}")
                AlarmScheduler.syncAll(context.applicationContext)
            }
        }
    }
}
