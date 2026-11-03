/* Whole-buffer export shares mouse selection wrapping and Unicode rules. */
static void
check_selection_export(void)
{
	TermSession *previous = tsessioncurrent();
	TermSession *created = tsessionnew(6, 2);
	const char *text = "abcdefgh\r\n\303\251\344\270\255";
	twrite(text, strlen(text), 0);
	assert(term.histlen == 1);
	Arg up = {.i = 1};
	kscrollup(&up);
	selstart(1, 0, 0);
	selextend(3, 0, SEL_REGULAR, 0);
	selextend(3, 0, SEL_REGULAR, 1);
	Selection saved = sel;
	char *all = getall();
	assert(strcmp(all, "abcdefgh\n\303\251\344\270\255\n") == 0);
	assert(term.scr == 1);
	assert(memcmp(&saved, &sel, sizeof(sel)) == 0);
	free(all);
	/* Alternate screen exports only that screen, without main-screen history. */
	const char *alternate = "\033[?1049hALT";
	twrite(alternate, strlen(alternate), 0);
	all = getall();
	assert(strstr(all, "ALT") != NULL);
	assert(strstr(all, "abcdefgh") == NULL);
	free(all);
	tsessionremove(created);
	tsessionuse(previous);
}
