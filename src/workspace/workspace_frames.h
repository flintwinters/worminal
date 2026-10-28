/* Ordered screen replication to the live controller. */
#ifndef WORMINAL_WORKSPACE_FRAMES_H
#define WORMINAL_WORKSPACE_FRAMES_H

static void
send_frame(Client *client, Tab *tab, int full)
{
	if (!tab->session)
		return;
	SessionFrame frame = tsessionframe(tab->session);
	WireBuffer buffer = {0};
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
	for (int row = 0; row < frame.rows; row++) {
		buffer.len = 0;
		if (!full && !tsessionrowdirty(tab->session, row))
			continue;
		Line line = tsessionviewline(tab->session, row);
		wire_put_u32(&buffer, row);
		for (int col = 0; col < frame.cols; col++) {
			wire_put_u32(&buffer, line[col].u);
			wire_put_u32(&buffer, line[col].mode);
			wire_put_u32(&buffer, line[col].fg);
			wire_put_u32(&buffer, line[col].bg);
		}
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
