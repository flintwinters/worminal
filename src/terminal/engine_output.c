/* See LICENSE. Ephemeral bounded observation over the engine's existing cells. */
#include <string.h>
#include "engine_parser.h"

int
worminal_engine_output_position(WorminalEngine *engine, WorminalOutputPosition *position)
{
    if (!engine || !position) return WORMINAL_INVALID;
    uint64_t history = engine->term.mode & MODE_ALTSCREEN ? 0 : engine->term.histlen;
    if (engine->output_base < history) engine->output_base = history;
    *position = (WorminalOutputPosition){engine->output_epoch,
        engine->output_base + engine->term.c.y, engine->term.c.x + !!(engine->term.c.state & CURSOR_WRAPNEXT)};
    return WORMINAL_OK;
}

int
worminal_engine_output(WorminalEngine *engine, WorminalOutputPosition start, WorminalOutputPosition end,
                      uint8_t *output, size_t capacity, size_t *bytes,
                      WorminalOutputPosition *next, uint32_t *flags)
{
    if (!engine || !output || !bytes || !next || !flags) return WORMINAL_INVALID;
    Term *t = &engine->term;
    uint64_t history = t->mode & MODE_ALTSCREEN ? 0 : t->histlen;
    /* A reflow can create history without a scroll; old epochs are already invalidated. */
    if (engine->output_base < history) engine->output_base = history;
    uint64_t first = engine->output_base - history;
    uint64_t last = engine->output_base + t->row - 1;
    *bytes = *flags = 0;
    if (end.epoch != engine->output_epoch || end.row > last || end.column < 0 || end.column > t->col) {
        *flags = 1;
        worminal_engine_output_position(engine, next);
        return WORMINAL_OK;
    }
    if (start.epoch != engine->output_epoch || start.row < first || start.row > end.row ||
        start.column < 0 || start.column > t->col || (start.row == end.row && start.column > end.column)) {
        *flags = 1;
        start = (WorminalOutputPosition){engine->output_epoch, first, 0};
    }
    *next = start;
    unsigned rows = 0;
    for (uint64_t row = start.row; row <= end.row; row++) {
        int begin = row == start.row ? start.column : 0;
        int limit = row == end.row ? end.column : t->col;
        if (row == end.row && begin == limit) { *next = end; break; }
        if (rows == 1000 || capacity - *bytes < (size_t)t->col * 4 + 1) { *flags |= 2; break; }
        int y = (int)(row - first) - (int)history;
        Line line = y >= 0 ? t->line[y] : t->hist[(t->histi + y + te_history_size) % te_history_size];
        while (limit > begin && line[limit - 1].u == ' ') limit--;
        for (int x = begin; x < limit; x++) {
            if (line[x].mode & ATTR_WDUMMY) continue;
            char encoded[4];
            size_t size = utf8encode(line[x].u ? line[x].u : ' ', encoded);
            memcpy(output + *bytes, encoded, size);
            *bytes += size;
        }
        output[(*bytes)++] = '\n';
        rows++;
        *next = row == end.row ? end : (WorminalOutputPosition){end.epoch, row + 1, 0};
    }
    return WORMINAL_OK;
}
