package com.seongjae.ir_remote

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import kotlin.concurrent.thread

/**
 * Fires at an alarm's scheduled time, sends its command, and arms tomorrow.
 *
 * This runs with the app process dead, so it cannot call into the Rust core.
 * The request logic below is therefore a SECOND implementation of the rules in
 * `api_client.rs`, and the two must be kept in step:
 *
 *  - exactly one attempt, never retried (rule 5) - every command fires an IR
 *    code, and `/Power` is a toggle, so a retry would actuate twice
 *  - an explicit timeout on connect and read (rule 4)
 *  - only a positive confirmation counts as success (rule 1); a timeout, a
 *    non-2xx, or a body it cannot read is recorded as failed, never as fired
 *
 * The outcome is written back into the same `alarms.json` the Rust core reads,
 * so the next time the app opens the UI shows what actually happened.
 */
class AlarmReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getLongExtra(AlarmScheduler.EXTRA_ID, -1L)
        if (id < 0) return

        val appContext = context.applicationContext
        // onReceive must return in ~10s; goAsync lets the work outlive it.
        val pending = goAsync()

        thread(name = "ir-alarm-$id") {
            try {
                fire(appContext, id)
            } catch (e: Exception) {
                Log.e(TAG, "alarm $id failed: $e")
                AlarmStore.recordOutcome(appContext, id, "failed", AlarmStore.localDay())
            } finally {
                // Re-arm tomorrow whatever happened, so one bad night does not
                // silently end the schedule.
                rescheduleTomorrow(appContext, id)
                pending.finish()
            }
        }
    }

    private fun fire(context: Context, id: Long) {
        val alarm = AlarmStore.readAlarms(context).firstOrNull { it.id == id }
        if (alarm == null) {
            Log.w(TAG, "alarm $id no longer exists")
            return
        }
        if (!alarm.enabled) {
            Log.i(TAG, "alarm $id is disabled")
            return
        }

        val baseUrl = AlarmStore.baseUrl(context)
        if (baseUrl.isNullOrBlank()) {
            Log.e(TAG, "no device address configured")
            AlarmStore.recordOutcome(context, id, "failed", AlarmStore.localDay())
            return
        }

        val url = baseUrl.trimEnd('/') + AlarmStore.commandPath(alarm.command)
        val ok = sendOnce(url, AlarmStore.timeoutMs(context))

        AlarmStore.recordOutcome(
            context,
            id,
            if (ok) "fired" else "failed",
            AlarmStore.localDay(),
        )
        Log.i(TAG, "alarm $id sent $url -> ${if (ok) "confirmed" else "not confirmed"}")
    }

    /**
     * One attempt. Returns true only on a positive confirmation.
     *
     * Mirrors `parse_body` in api_client.rs: the JSON contract
     * `{"result":"Success",...}` or a bare `success`, case-insensitively.
     * Anything else is not a success.
     */
    private fun sendOnce(url: String, timeoutMs: Int): Boolean {
        var conn: HttpURLConnection? = null
        return try {
            conn = (URL(url).openConnection() as HttpURLConnection).apply {
                requestMethod = "GET"
                connectTimeout = timeoutMs
                readTimeout = timeoutMs
                useCaches = false
            }
            val code = conn.responseCode
            if (code !in 200..299) {
                Log.w(TAG, "HTTP $code from $url")
                return false
            }
            val body = conn.inputStream.bufferedReader().use { it.readText() }.trim()
            confirms(body)
        } catch (e: Exception) {
            // Timeout, refused, unreachable: not a confirmation.
            Log.w(TAG, "send failed for $url: $e")
            false
        } finally {
            conn?.disconnect()
        }
    }

    private fun confirms(body: String): Boolean {
        // Shape 1: the JSON contract.
        try {
            val o = JSONObject(body)
            return o.optString("result").equals("Success", ignoreCase = true)
        } catch (_: Exception) {
            // Not JSON; fall through to the bare-word shape.
        }
        // Shape 2: what Main.ino writes, `client.println("success")`.
        return body.equals("success", ignoreCase = true)
    }

    private fun rescheduleTomorrow(context: Context, id: Long) {
        val alarm = AlarmStore.readAlarms(context).firstOrNull { it.id == id } ?: return
        if (alarm.enabled) AlarmScheduler.schedule(context, alarm)
    }

    companion object {
        private const val TAG = "AlarmReceiver"
    }
}
