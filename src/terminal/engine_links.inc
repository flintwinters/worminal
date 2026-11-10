/* See LICENSE. OSC 8 hyperlinks: cells carry a 16-bit id into a bounded per-terminal URI table. */

#define LINK_URI_MAX 2048        /* longer targets are ignored, like malformed ones */
#define LINK_TABLE_BYTES 262144  /* URI bytes retained per terminal */

void
te_links_free(TermEngine *session)
{
	for (uint16_t i = 0; i < session->link_count; i++)
		free(session->links[i]);
	free(session->links);
	session->links = NULL;
	session->link_count = 0;
	session->link_bytes = 0;
}

static void
te_links_mark(Line row, int col, uint8_t *used)
{
	if (row)
		for (int x = 0; x < col; x++)
			used[row[x].link] = 1;
}

/*
 * Ids stay valid while any retained cell (screen, alternate screen, history) or cursor
 * state references them. Reclaiming only unreferenced slots lets long sessions that
 * print many distinct links keep working without renumbering cells.
 */
static void
te_links_collect(TermEngine *session)
{
	uint8_t *used = te_alloc(session->link_count);
	memset(used, 0, session->link_count);
	for (int y = 0; y < term.row; y++) {
		te_links_mark(term.line[y], term.col, used);
		if (term.alt)
			te_links_mark(term.alt[y], term.col, used);
	}
	if (term.hist)
		for (int y = 0; y < theme_history_size; y++)
			te_links_mark(term.hist[y], term.col, used);
	used[term.c.attr.link] = used[session->saved[0].attr.link] = used[session->saved[1].attr.link] = 1;
	for (uint16_t i = 1; i < session->link_count; i++) {
		if (used[i] || !session->links[i])
			continue;
		session->link_bytes -= strlen(session->links[i]) + 1;
		free(session->links[i]);
		session->links[i] = NULL;
	}
	free(used);
}

/* Returns the id for uri, or 0 when the table cannot hold it. */
static uint16_t
te_link_intern(TermEngine *session, const char *uri)
{
	size_t bytes = strlen(uri) + 1;
	uint16_t slot;

	for (uint16_t i = 1; i < session->link_count; i++)
		if (session->links[i] && !strcmp(session->links[i], uri))
			return i;
	for (int pass = 0;; pass++) {
		slot = 0;
		for (uint16_t i = 1; i < session->link_count && !slot; i++)
			slot = session->links[i] ? 0 : i;
		if (session->link_bytes + bytes <= LINK_TABLE_BYTES &&
		    (slot || session->link_count < UINT16_MAX))
			break;
		if (pass)
			return 0;
		te_links_collect(session);
	}
	if (!slot) {
		if (!session->link_count) { /* slot 0 means "no link" */
			session->links = te_realloc(session->links, sizeof(char *));
			session->links[0] = NULL;
			session->link_count = 1;
		}
		session->links = te_realloc(session->links, ((size_t)session->link_count + 1) * sizeof(char *));
		slot = session->link_count++;
	}
	session->links[slot] = te_strdup(uri);
	session->link_bytes += bytes;
	return slot;
}

/* OSC 8 ; params ; URI. An empty URI closes the link; the parameters (such as id=) are ignored. */
static void
te_oschyperlink(TermEngine *session)
{
	char *uri, *end = strescseq.buf + strescseq.len;

	if (strescseq.narg < 3) {
		term.c.attr.link = 0;
		return;
	}
	/* te_strparse split the target at its own semicolons; restore them. */
	uri = strescseq.args[2];
	for (char *p = uri; p < end; p++)
		if (!*p)
			*p = ';';
	term.c.attr.link = (*uri && strlen(uri) <= LINK_URI_MAX) ? te_link_intern(session, uri) : 0;
}
