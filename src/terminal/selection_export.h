/* Private selection export; included after the shared text serializer. */
/* Export the retained buffer using exactly the normal selection text rules.
 * Negative rows address history with a zero scroll offset. Preserve the view. */
char *
getall(void)
{
	Selection saved = sel;
	int savedscroll = term.scr;
	int first = IS_SET(MODE_ALTSCREEN) ? 0 : -term.histlen;
	term.scr = 0;
	sel = (Selection){.type = SEL_REGULAR,
	    .ob = {0, first}, .nb = {0, first},
	    .oe = {term.col - 1, term.row - 1}, .ne = {term.col - 1, term.row - 1}};
	char *text = getsel();
	sel = saved;
	term.scr = savedscroll;
	return text;
}
