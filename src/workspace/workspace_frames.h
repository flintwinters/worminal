/* Ordered screen replication to the live controller. */
#ifndef WORMINAL_WORKSPACE_FRAMES_H
#define WORMINAL_WORKSPACE_FRAMES_H

static void
frame_encode_row(WireBuffer *buffer, Line line, int cols)
{
	for (int col = 0; col < cols; col++) {
		wire_put_u32(buffer, line[col].u);
		wire_put_u32(buffer, line[col].mode);
		wire_put_u32(buffer, line[col].fg);
		wire_put_u32(buffer, line[col].bg);
	}
}

static uint64_t
frame_row_hash(const unsigned char *bytes, size_t count)
{
	uint64_t hash = UINT64_C(14695981039346656037);
	for (size_t i = 0; i < count; i++) hash = (hash ^ bytes[i]) * UINT64_C(1099511628211);
	return hash;
}

/* Find a contiguous row move against the last queued viewport. Hashes only
 * select candidates: exact comparison below owns replication correctness. */
static int
frame_copy_rows(Client *client, Tab *tab, SessionFrame frame, WireBuffer *current)
{
	int dirty = 0, rows = frame.rows;
	for (int row = 0; row < rows; row++) dirty += !!tsessionrowdirty(tab->session, row);
	if (dirty < 3 || dirty * 2 < rows) return 0;
	size_t width = (size_t)frame.cols * WIRE_CELL_BYTES;
	uint64_t old[rows], now[rows];
	int changed[rows];
	for (int row = 0; row < rows; row++) {
		frame_encode_row(current, tsessionviewline(tab->session, row), frame.cols);
		old[row] = frame_row_hash(tab->screen.data + row * width, width);
		now[row] = frame_row_hash(current->data + row * width, width);
		changed[row] = old[row] != now[row] ||
		    memcmp(current->data + row * width, tab->screen.data + row * width, width);
	}
	int best_score = 1, source = 0, destination = 0, count = 0;
	for (int shift = 1 - rows; shift < rows; shift++) {
		if (!shift) continue;
		int run = 0, score = 0;
		for (int row = 0; row < rows; row++) {
			int from = row + shift;
			if (from < 0 || from >= rows || now[row] != old[from]) { run = score = 0; continue; }
			run++;
			score += !!changed[row];
			if (score > best_score || (score == best_score && count && run > count)) {
				best_score = score; destination = row - run + 1;
				source = destination + shift; count = run;
			}
		}
	}
	if (!count) return 0;
	for (int row = 0; row < count; row++)
		if (memcmp(current->data + (destination + row) * width, tab->screen.data + (source + row) * width, width))
			return 0;
	WireBuffer move = {0};
	wire_put_u32(&move, source); wire_put_u32(&move, destination); wire_put_u32(&move, count);
	int queued = client_queue(client, WIRE_COPY_ROWS, tab->id, &move);
	wire_buffer_free(&move);
	if (!queued) return -1;
	memmove(tab->screen.data + destination * width, tab->screen.data + source * width, count * width);
	return 1;
}

static void
send_frame(Client *client, Tab *tab, int full)
{
	if (!tab->session)
		return;
	SessionFrame frame = tsessionframe(tab->session);
	WireBuffer buffer = {0}, current = {0};
	if (!client || client->fd < 0 || !client->ready)
		return;
	if (client->output.len - client->sent > 1024u * 1024u) {
		client->needs_resync = 1;
		return;
	}
	size_t row_bytes = (size_t)frame.cols * WIRE_CELL_BYTES;
	full |= tab->screen_cols != frame.cols || tab->screen.len != row_bytes * frame.rows;
	if (full) {
		tab->screen.len = 0;
		tab->screen_cols = frame.cols;
	}
	wire_put_u32(&buffer, frame.cols);
	wire_put_u32(&buffer, frame.rows);
	wire_put_u32(&buffer, frame.cursor_x);
	wire_put_u32(&buffer, frame.cursor_y);
	wire_put_u32(&buffer, frame.scroll);
	wire_put_u32(&buffer, frame.alternate);
	/* Queue a complete frame before waking SSH; row writes can fragment compression. */
	if (!client_queue(client, WIRE_FRAME, tab->id, &buffer))
		goto done;
	if (!full && (client->features & WIRE_FEATURE_COPY_ROWS) && frame_copy_rows(client, tab, frame, &current) < 0)
		goto done;
	for (int row = 0; row < frame.rows; row++) {
		buffer.len = 0;
		if (!full && !tsessionrowdirty(tab->session, row))
			continue;
		wire_put_u32(&buffer, row);
		if (current.len) wire_put_bytes(&buffer, current.data + row * row_bytes, row_bytes);
		else frame_encode_row(&buffer, tsessionviewline(tab->session, row), frame.cols);
		const unsigned char *cells = buffer.data + sizeof(uint32_t);
		/* Erase/redraw operations can leave identical cells dirty. Advance the
		 * exact wire baseline only after queueing: skipped frames must not make
		 * future deltas depend on undelivered cells. Release discards the cache. */
		if (!full && !memcmp(tab->screen.data + row * row_bytes, cells, row_bytes))
			continue;
		if (!client_queue(client, WIRE_ROW, tab->id, &buffer))
			goto done;
		if (full) wire_put_bytes(&tab->screen, cells, row_bytes);
		else memcpy(tab->screen.data + row * row_bytes, cells, row_bytes);
	}
	client_send(client, WIRE_FRAME_FINISH, tab->id, NULL);
done:
	wire_buffer_free(&current);
	wire_buffer_free(&buffer);
}

static void
flush_frame(Tab *tab)
{
	send_frame(tab->controller, tab, 0);
	tsessioncleandirty(tab->session);
	tab->pending_frame = 0;
}

#endif
