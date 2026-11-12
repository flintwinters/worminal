/* See LICENSE. OSC 99 desktop notifications (kitty protocol): chunks assemble one bounded pending notification. */

#define NOTIFY_ID_MAX 64       /* identifiers are echoed in query replies; longer ones are malformed */
#define NOTIFY_TITLE_MAX 256   /* text beyond these bounds is dropped at a UTF-8 boundary */
#define NOTIFY_BODY_MAX 2048

void
te_notify_free(TermEngine *session)
{
	free(session->notify.id);
	free(session->notify.title);
	free(session->notify.body);
	memset(&session->notify, 0, sizeof(session->notify));
}

static void
te_notify_append(char **text, const char *chunk, size_t max)
{
	size_t used = *text ? strlen(*text) : 0, bytes = strlen(chunk);

	if (bytes > max - used) {
		bytes = max - used;
		while (bytes && ((unsigned char)chunk[bytes] & 0xC0) == 0x80)
			bytes--;
	}
	*text = te_realloc(*text, used + bytes + 1);
	memcpy(*text + used, chunk, bytes);
	(*text)[used + bytes] = '\0';
}

/*
 * OSC 99 ; key=value:... ; payload. Title and body arrive as separate chunks sharing an
 * identifier; d=0 announces more. Icons, buttons, sounds, close requests and activation
 * reports are not implemented, and the p=? reply says so. Identifiers reach the PTY in that
 * reply, so anything outside the protocol's alphabet discards the sequence.
 */
static void
te_oscnotify(TermEngine *session)
{
	enum { TITLE, BODY, QUERY, OTHER } type = TITLE;
	char *meta = strescseq.narg > 1 ? strescseq.args[1] : NULL, *payload = NULL, *decoded = NULL;
	char *end = strescseq.buf + strescseq.len, *value;
	const char *id = "0";
	int done = 1, encoded = 0, urgency = -1, occasion = -1;

	if (strescseq.narg > 2) {
		/* te_strparse split the payload at its own semicolons; restore them. */
		payload = strescseq.args[2];
		for (char *p = payload; p < end; p++)
			if (!*p)
				*p = ';';
	}
	for (char *key = meta, *next; key && *key; key = next) {
		if ((next = strchr(key, ':')))
			*next++ = '\0';
		if (!key[0] || key[1] != '=')
			continue;
		value = key + 2;
		switch (key[0]) {
		case 'i':
			if (!*value || strlen(value) > NOTIFY_ID_MAX ||
			    value[strspn(value, "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-+.")])
				return;
			id = value;
			break;
		case 'p':
			type = !strcmp(value, "title") ? TITLE : !strcmp(value, "body") ? BODY :
			       !strcmp(value, "?") ? QUERY : OTHER;
			/* Layout chunks carry no text; unknown requests must not complete a notification. */
			if (type == OTHER && strcmp(value, "icon") && strcmp(value, "buttons"))
				return;
			break;
		case 'd': done = strcmp(value, "0") != 0; break;
		case 'e': encoded = !strcmp(value, "1"); break;
		case 'u':
			if (value[0] >= '0' && value[0] <= '2' && !value[1])
				urgency = value[0] - '0';
			break;
		case 'o':
			occasion = !strcmp(value, "unfocused") ? WORMINAL_NOTIFY_UNFOCUSED :
			           !strcmp(value, "invisible") ? WORMINAL_NOTIFY_INVISIBLE :
			           !strcmp(value, "always") ? WORMINAL_NOTIFY_ALWAYS : occasion;
			break;
		}
	}
	if (type == QUERY) {
		char reply[NOTIFY_ID_MAX + 96];
		int bytes = snprintf(reply, sizeof(reply),
			"\033]99;i=%s:p=?;a=focus:o=always,unfocused,invisible:p=title,body,?:u=0,1,2\033\\", id);
		te_ttywrite(session, reply, bytes, 0);
		return;
	}
	/* One notification is assembled at a time: another identifier abandons the unfinished one. */
	if (!session->notify.id || strcmp(session->notify.id, id)) {
		te_notify_free(session);
		session->notify.id = te_strdup(id);
		session->notify.urgency = 1;
		session->notify.occasion = WORMINAL_NOTIFY_ALWAYS;
	}
	if (urgency >= 0)
		session->notify.urgency = urgency;
	if (occasion >= 0)
		session->notify.occasion = occasion;
	if (payload && type != OTHER) {
		if (encoded)
			payload = decoded = base64dec(payload);
		te_notify_append(type == TITLE ? &session->notify.title : &session->notify.body, payload,
		                 type == TITLE ? NOTIFY_TITLE_MAX : NOTIFY_BODY_MAX);
		free(decoded);
	}
	if (!done)
		return;
	{
		const char *title = session->notify.title ? session->notify.title : "";
		const char *body = session->notify.body ? session->notify.body : "";
		size_t title_bytes = strlen(title), body_bytes = strlen(body);
		if (title_bytes + body_bytes) {
			char text[NOTIFY_TITLE_MAX + NOTIFY_BODY_MAX];
			memcpy(text, title, title_bytes);
			memcpy(text + title_bytes, body, body_bytes);
			te_emit(session, WORMINAL_NOTIFY, (int32_t)title_bytes,
			        (uint32_t)session->notify.urgency | (uint32_t)session->notify.occasion << 8,
			        text, title_bytes + body_bytes);
		}
	}
	te_notify_free(session);
}
