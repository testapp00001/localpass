package app.localpass

import android.app.assist.AssistStructure
import android.text.InputType
import android.view.View
import android.view.autofill.AutofillId
import android.view.autofill.AutofillValue

/** What a login form looks like in the system's view of a page: which site, and which fields. */
internal object AutofillParser {
    class Form(val origin: String, val username: AutofillId?, val password: AutofillId?) {
        val ids: Array<AutofillId> get() = listOfNotNull(username, password).toTypedArray()

        /**
         * Only a form with both a username and a password field is offered for saving: a lone
         * password field is a later step of a login, or a search box mistaken for one.
         */
        val savable: Boolean get() = username != null && password != null
    }

    /** The website a saved screen reports, the way [parse] and [typed] read it. Null when it reports none. */
    fun originOf(structure: AssistStructure): String? = scan(structure, visibleOnly = false)?.origin

    /**
     * What the user typed into a login form, read from the structure the system hands to
     * `onSaveRequest`. Not a data class on purpose: nothing should be able to print the password.
     */
    class Typed(val origin: String, val username: String, val password: CharArray)

    private class Field(val id: AutofillId, val kind: Kind, val focused: Boolean, val value: AutofillValue?)

    private enum class Kind { PASSWORD, USERNAME, TEXT }

    private class Scan(val host: String, val scheme: String?, val fields: List<Field>) {
        val origin: String get() = "${scheme?.takeIf { it.isNotBlank() } ?: "https"}://$host"
    }

    /** The two fields of a login form: (username, password). */
    private class Pick(val username: Field?, val password: Field?)

    /**
     * Finds the login form in a screen, or null when there is none or the page's website is not
     * known (native apps report no website, so LocalPass has nothing to match them against).
     */
    fun parse(structure: AssistStructure): Form? {
        val scan = scan(structure, visibleOnly = true) ?: return null
        val pick = pick(scan.fields)
        if (pick.password == null && pick.username == null) return null
        return Form(scan.origin, pick.username?.id, pick.password?.id)
    }

    /**
     * The username and password typed into the login form of a saved screen, for the same website
     * [parse] would report. Null when there is no complete form or either value is empty.
     */
    fun typed(structure: AssistStructure): Typed? {
        // By the time the system saves, the page may already have hidden its form again, so a
        // field that is no longer visible still counts when no visible password field is left.
        val visible = scan(structure, visibleOnly = true)
        val scan = visible?.takeIf { pick(it.fields).password != null } ?: scan(structure, visibleOnly = false) ?: return null
        val pick = pick(scan.fields)
        val user = pick.username?.let { text(it.value) }?.toString()?.trim()
        val password = pick.password?.let { text(it.value) }
        if (user.isNullOrEmpty() || password.isNullOrEmpty()) return null
        return Typed(scan.origin, user, CharArray(password.length) { password[it] })
    }

    private fun text(value: AutofillValue?): CharSequence? = value?.takeIf { it.isText }?.textValue

    private fun scan(structure: AssistStructure, visibleOnly: Boolean): Scan? {
        var domain: String? = null
        var scheme: String? = null
        val fields = mutableListOf<Field>()

        fun visit(node: AssistStructure.ViewNode) {
            if (domain == null && !node.webDomain.isNullOrBlank()) {
                domain = node.webDomain
                scheme = node.webScheme
            }
            val id = node.autofillId
            if (id != null && node.autofillType == View.AUTOFILL_TYPE_TEXT && (!visibleOnly || node.visibility == View.VISIBLE)) {
                classify(node)?.let { fields.add(Field(id, it, node.isFocused, node.autofillValue)) }
            }
            for (i in 0 until node.childCount) visit(node.getChildAt(i))
        }
        for (i in 0 until structure.windowNodeCount) visit(structure.getWindowNodeAt(i).rootViewNode)

        return Scan(domain ?: return null, scheme, fields)
    }

    private fun pick(fields: List<Field>): Pick {
        // The field the user is in, or else the first password field, anchors the form.
        val password = fields.firstOrNull { it.kind == Kind.PASSWORD && it.focused } ?: fields.firstOrNull { it.kind == Kind.PASSWORD }
        val before = if (password != null) fields.subList(0, fields.indexOf(password)) else fields
        val username = before.lastOrNull { it.kind == Kind.USERNAME } ?: before.lastOrNull { it.kind == Kind.TEXT && password != null }
            ?: fields.firstOrNull { it.kind == Kind.USERNAME && it.focused }
        return Pick(username, password)
    }

    private fun attribute(node: AssistStructure.ViewNode, name: String): String? =
        node.htmlInfo?.attributes?.firstOrNull { it.first.equals(name, ignoreCase = true) }?.second?.lowercase()

    private fun classify(node: AssistStructure.ViewNode): Kind? {
        val hints = node.autofillHints.orEmpty().map { it.lowercase() }
        val type = attribute(node, "type")
        val autocomplete = attribute(node, "autocomplete").orEmpty()
        val variation = node.inputType and InputType.TYPE_MASK_VARIATION
        val isTextClass = node.inputType and InputType.TYPE_MASK_CLASS == InputType.TYPE_CLASS_TEXT
        val password = View.AUTOFILL_HINT_PASSWORD in hints || type == "password" ||
            "current-password" in autocomplete || "new-password" in autocomplete ||
            (isTextClass && (variation == InputType.TYPE_TEXT_VARIATION_PASSWORD ||
                variation == InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD ||
                variation == InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD))
        if (password) return Kind.PASSWORD
        if (View.AUTOFILL_HINT_USERNAME in hints || View.AUTOFILL_HINT_EMAIL_ADDRESS in hints ||
            "username" in autocomplete || "email" in autocomplete
        ) {
            return Kind.USERNAME
        }
        // Plain text-like inputs; checkboxes, buttons and search boxes are not login fields.
        val textLike = type == null || type in setOf("text", "email", "tel", "")
        val isInput = node.htmlInfo?.tag?.equals("input", ignoreCase = true) ?: (node.className?.contains("EditText") == true)
        return if (isInput && textLike && "search" !in autocomplete) Kind.TEXT else null
    }
}
