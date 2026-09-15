package com.seongjae.ir_remote

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.Calendar

/**
 * Shared state between the Rust core and the alarm receiver.
 *
 * The alarms themselves live in the SAME `alarms.json` the Rust core reads and
 * writes, so there is one format and one source of truth. Only the two things
 * the receiver needs in order to find that file and reach the device are kept
 * in SharedPreferences, because the receiver runs when the app process - and
 * therefore the Rust core - is not alive.
 *
 * Concurrent access is possible in principle (an alarm firing while the app is
 * open) but the window is tiny and both sides rewrite the whole file. If alarms
 * ever grow beyond this toy scale, this wants a real lock.
 */
object AlarmStore {
    private const val PREFS = "ir_remote_alarms"
    private const val KEY_PATH = "alarms_path"
    private const val KEY_BASE_URL = "base_url"
    private const val KEY_TIMEOUT = "command_timeout_ms"

    fun save(context: Context, alarmsPath: String, baseUrl: String, timeoutMs: Int) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_PATH, alarmsPath)
            .putString(KEY_BASE_URL, baseUrl)
            .putInt(KEY_TIMEOUT, timeoutMs)
            .apply()
    }

    fun alarmsPath(context: Context): String? =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_PATH, null)

    fun baseUrl(context: Context): String? =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_BASE_URL, null)

    fun timeoutMs(context: Context): Int =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getInt(KEY_TIMEOUT, 3000)

    // ------------------------------------------------------------ alarms ----

    data class Alarm(
        val id: Long,
        val hour: Int,
        val minute: Int,
        val enabled: Boolean,
        val command: String,
        val lastDay: Long?,
    )

    /** Path segment of the command, matching `Command::path()` in state.rs. */
    fun commandPath(command: String): String = when (command) {
        "Power" -> "/Power"
        "Silent" -> "/Silent"
        "LowTemp" -> "/Low_Temp"
        "HighTemp" -> "/High_Temp"
        else -> "/$command"
    }

    fun readAlarms(context: Context): List<Alarm> {
        val path = alarmsPath(context) ?: return emptyList()
        val file = File(path)
        if (!file.exists()) return emptyList()

        return try {
            val array = JSONArray(file.readText())
            (0 until array.length()).map { i ->
                val o = array.getJSONObject(i)
                Alarm(
                    id = o.getLong("id"),
                    hour = o.getInt("hour"),
                    minute = o.getInt("minute"),
                    enabled = o.optBoolean("enabled", true),
                    command = o.optString("command", "Power"),
                    lastDay = if (o.isNull("lastDay")) null else o.getLong("lastDay"),
                )
            }
        } catch (e: Exception) {
            Log.e("AlarmStore", "could not read $path: $e")
            emptyList()
        }
    }

    /**
     * Record how an alarm resolved, in the same shape `alarm.rs` deserialises.
     *
     * `lastFiredMs` is set ONLY on "fired". Writing it for a failed send would
     * make the UI claim a command reached the device when it never did - the
     * same rule-1 lie the desktop app is built to avoid.
     */
    fun recordOutcome(context: Context, id: Long, outcome: String, localDay: Long) {
        val path = alarmsPath(context) ?: return
        val file = File(path)
        if (!file.exists()) return

        try {
            val array = JSONArray(file.readText())
            for (i in 0 until array.length()) {
                val o = array.getJSONObject(i)
                if (o.getLong("id") != id) continue
                o.put("lastDay", localDay)
                o.put("lastOutcome", outcome)
                if (outcome == "fired") {
                    o.put("lastFiredMs", System.currentTimeMillis())
                }
                break
            }
            file.writeText(array.toString(2))
        } catch (e: Exception) {
            Log.e("AlarmStore", "could not record outcome: $e")
        }
    }

    /** Local day number, matching `alarm::local_now` in the Rust core. */
    fun localDay(): Long {
        val cal = Calendar.getInstance()
        val offsetMs = (cal.get(Calendar.ZONE_OFFSET) + cal.get(Calendar.DST_OFFSET)).toLong()
        return Math.floorDiv(System.currentTimeMillis() + offsetMs, 86_400_000L)
    }

    private object Log {
        fun e(tag: String, msg: String) = android.util.Log.e(tag, msg)
    }

    fun toJsonArray(alarms: List<Alarm>): JSONArray {
        val a = JSONArray()
        alarms.forEach {
            a.put(
                JSONObject()
                    .put("id", it.id)
                    .put("hour", it.hour)
                    .put("minute", it.minute)
                    .put("enabled", it.enabled)
                    .put("command", it.command)
            )
        }
        return a
    }
}
