package com.seongjae.ir_remote

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import java.util.Calendar

/**
 * Arms the next occurrence of each alarm with AlarmManager.
 *
 * Why AlarmManager and not the Rust tokio scheduler: Android suspends and kills
 * backgrounded processes, and Doze defers work, so a tokio timer stops firing
 * the moment the app leaves the screen. `setExactAndAllowWhileIdle` is the only
 * API that wakes the device at a wall-clock time while it is dozing.
 *
 * Each alarm is one-shot and reschedules itself after firing, because the
 * exact-while-idle APIs have no repeating variant.
 */
object AlarmScheduler {
    private const val TAG = "AlarmScheduler"
    const val EXTRA_ID = "alarm_id"

    private fun manager(context: Context): AlarmManager =
        context.getSystemService(Context.ALARM_SERVICE) as AlarmManager

    private fun intentFor(context: Context, id: Long): PendingIntent =
        PendingIntent.getBroadcast(
            context,
            id.toInt(),
            Intent(context, AlarmReceiver::class.java).putExtra(EXTRA_ID, id),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    /**
     * Whether the system will honour an exact alarm.
     *
     * On Android 12+ this can be revoked by the user, and on 13+ it must be
     * granted in system settings unless the app holds USE_EXACT_ALARM. When it
     * is false the alarm is still scheduled, but inexactly - it may drift by
     * minutes, which the UI should say rather than silently promising 07:00.
     */
    fun canScheduleExact(context: Context): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            manager(context).canScheduleExactAlarms()
        } else {
            true
        }

    /** Cancel everything, then arm the next occurrence of each enabled alarm. */
    fun syncAll(context: Context) {
        val alarms = AlarmStore.readAlarms(context)
        for (a in alarms) {
            cancel(context, a.id)
            if (a.enabled) schedule(context, a)
        }
        Log.i(TAG, "synced ${alarms.count { it.enabled }} enabled alarm(s)")
    }

    fun cancel(context: Context, id: Long) {
        manager(context).cancel(intentFor(context, id))
    }

    fun schedule(context: Context, alarm: AlarmStore.Alarm) {
        val at = nextOccurrence(alarm.hour, alarm.minute)
        val pending = intentFor(context, alarm.id)
        val am = manager(context)

        try {
            if (canScheduleExact(context)) {
                // Fires even in Doze. This is the whole point of the feature.
                am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
            } else {
                // Permission withheld: still schedule, but the OS may batch it.
                am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
                Log.w(TAG, "exact alarms not permitted - alarm ${alarm.id} may drift")
            }
        } catch (e: SecurityException) {
            // Losing the permission between the check and the call is possible.
            Log.e(TAG, "could not schedule alarm ${alarm.id}: $e")
        }
    }

    /**
     * The next time this wall clock occurs: today if it is still ahead of us,
     * otherwise tomorrow.
     *
     * Deliberately never returns a time in the past. Firing a Power toggle late
     * is worse than not firing it - the same rule the Rust scheduler follows
     * when it marks an alarm missed.
     */
    fun nextOccurrence(hour: Int, minute: Int, now: Long = System.currentTimeMillis()): Long {
        val cal = Calendar.getInstance().apply {
            timeInMillis = now
            set(Calendar.HOUR_OF_DAY, hour)
            set(Calendar.MINUTE, minute)
            set(Calendar.SECOND, 0)
            set(Calendar.MILLISECOND, 0)
        }
        if (cal.timeInMillis <= now) {
            cal.add(Calendar.DAY_OF_YEAR, 1)
        }
        return cal.timeInMillis
    }
}
